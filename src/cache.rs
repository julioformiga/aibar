use crate::model::{Provider, SourceState};
use crate::theme::Theme;
use directories::BaseDirs;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::{fs, io, path::PathBuf};

#[derive(Serialize, Deserialize, Default)]
pub struct CachedState {
    #[serde(default)]
    pub active_tab: usize,
    #[serde(default)]
    pub active_sources: HashMap<String, usize>,
    #[serde(default)]
    pub sources: Vec<CachedSource>,
    #[serde(default)]
    pub theme: Theme,
}

#[derive(Serialize, Deserialize)]
pub struct CachedSource {
    pub provider: Provider,
    pub source_id: String,
    pub state: SourceState,
}

pub struct Cache {
    path: PathBuf,
}

impl Cache {
    pub fn new() -> io::Result<Self> {
        let base = BaseDirs::new()
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "could not resolve home dir"))?;
        let dir = base.cache_dir().join("aibar");
        fs::create_dir_all(&dir)?;
        Ok(Self {
            path: dir.join("state.json"),
        })
    }

    pub fn load(&self) -> Option<CachedState> {
        let data = fs::read(&self.path).ok()?;
        serde_json::from_slice(&data).ok()
    }

    pub fn save(&self, state: &CachedState) -> io::Result<()> {
        let data = serde_json::to_vec_pretty(state).map_err(io::Error::other)?;
        let tmp_path = self.path.with_extension("json.tmp");
        fs::write(&tmp_path, data)?;
        fs::rename(&tmp_path, &self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        Ceiling, CeilingReport, CreditsState, HyperPlan, LimitWindow, ProviderState, WindowKind,
    };
    use chrono::Utc;

    fn temp_cache_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aibar_test_{name}_{nanos}"));
        fs::create_dir_all(&dir).unwrap();
        dir.join("state.json")
    }

    #[test]
    fn load_returns_none_when_file_missing() {
        let cache = Cache {
            path: temp_cache_path("missing"),
        };
        assert!(cache.load().is_none());
    }

    #[test]
    fn save_then_load_round_trips_state() {
        let cache = Cache {
            path: temp_cache_path("roundtrip"),
        };

        let mut active_sources = HashMap::new();
        active_sources.insert("Claude".to_string(), 1usize);

        let state = CachedState {
            active_tab: 2,
            theme: Theme::Btop,
            active_sources,
            sources: vec![
                CachedSource {
                    provider: Provider::Claude,
                    source_id: "oauth".into(),
                    state: SourceState::Quota(ProviderState {
                        provider: Provider::Claude,
                        label: "Claude (Pro)".into(),
                        windows: vec![LimitWindow::from_values(
                            WindowKind::FiveHours,
                            None,
                            580,
                            1000,
                            None,
                        )],
                        last_updated: None,
                        last_error: None,
                    }),
                },
                CachedSource {
                    provider: Provider::Claude,
                    source_id: "api-key".into(),
                    state: SourceState::Ceiling(CeilingReport {
                        label: "Claude (API)".into(),
                        ceilings: vec![Ceiling {
                            group: "claude-sonnet-4".into(),
                            rpm: 50,
                            in_tpm: 100_000,
                            out_tpm: 20_000,
                        }],
                        last_updated: None,
                        last_error: None,
                    }),
                },
            ],
        };

        cache.save(&state).unwrap();
        let loaded = cache.load().expect("cache file should load");

        assert_eq!(loaded.active_tab, 2);
        assert_eq!(loaded.theme, Theme::Btop);
        assert_eq!(loaded.active_sources.get("Claude"), Some(&1));
        assert_eq!(loaded.sources.len(), 2);

        match &loaded.sources[0].state {
            SourceState::Quota(ps) => {
                assert_eq!(ps.label, "Claude (Pro)");
                assert_eq!(ps.windows.len(), 1);
                assert_eq!(ps.windows[0].used, 580);
            }
            SourceState::Ceiling(_) => panic!("expected Quota state"),
            SourceState::Credits(_) => panic!("expected Quota state"),
        }
        match &loaded.sources[1].state {
            SourceState::Ceiling(cr) => {
                assert_eq!(cr.ceilings[0].rpm, 50);
            }
            SourceState::Quota(_) => panic!("expected Ceiling state"),
            SourceState::Credits(_) => panic!("expected Ceiling state"),
        }

        let _ = fs::remove_dir_all(cache.path.parent().unwrap());
    }

    #[test]
    fn cached_state_default_is_empty() {
        let state = CachedState::default();
        assert_eq!(state.active_tab, 0);
        assert_eq!(state.theme, Theme::Default);
        assert!(state.active_sources.is_empty());
        assert!(state.sources.is_empty());
    }

    #[test]
    fn save_then_load_round_trips_credits_state() {
        let cache = Cache {
            path: temp_cache_path("credits"),
        };

        let state = CachedState {
            sources: vec![CachedSource {
                provider: Provider::Hyper,
                source_id: "default".into(),
                state: SourceState::Credits(CreditsState {
                    label: "Hyper".into(),
                    balance: Some(42.5),
                    plan: Some(HyperPlan::Monthly),
                    reset_at: Some(Utc::now()),
                    last_updated: None,
                    last_error: None,
                }),
            }],
            ..CachedState::default()
        };

        cache.save(&state).unwrap();
        let loaded = cache.load().expect("cache file should load");

        match &loaded.sources[0].state {
            SourceState::Credits(cs) => {
                assert_eq!(cs.label, "Hyper");
                assert_eq!(cs.balance, Some(42.5));
                assert_eq!(cs.plan, Some(HyperPlan::Monthly));
                assert!(cs.reset_at.is_some());
            }
            _ => panic!("expected Credits state"),
        }

        let _ = fs::remove_dir_all(cache.path.parent().unwrap());
    }
}
