use crate::agents::Agent;
use crate::config::{
    http_timeout, CLAUDE_API_RATE_LIMITS_URL, CLAUDE_API_VERSION, CLAUDE_OAUTH_USAGE_URL,
};
use crate::model::{
    Ceiling, CeilingReport, LimitWindow, Provider, ProviderState, SourceState, WindowKind,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::PathBuf;

// ===== OAuth Agent (Pro / Max) =====

pub struct ClaudeOAuthAgent {
    source_id: String,
    creds_path: PathBuf,
    config_path: Option<PathBuf>,
    client: reqwest::Client,
}

impl ClaudeOAuthAgent {
    pub fn detect_all() -> Vec<Self> {
        let mut agents = Vec::new();

        let creds_path = if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
            PathBuf::from(dir).join(".credentials.json")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".claude/.credentials.json")
        } else {
            return agents;
        };

        if !creds_path.exists() {
            return agents;
        }

        let config_path = creds_path
            .parent()
            .and_then(|p| p.parent())
            .map(|p| p.join(".claude.json"))
            .filter(|p| p.exists());

        let client = reqwest::Client::builder()
            .timeout(http_timeout())
            .build()
            .unwrap_or_default();

        agents.push(Self {
            source_id: "oauth".into(),
            creds_path,
            config_path,
            client,
        });

        agents
    }

    fn read_access_token(&self) -> anyhow::Result<String> {
        let data = std::fs::read_to_string(&self.creds_path)?;
        let v: Value = serde_json::from_str(&data)?;
        let token = v["claudeAiOauth"]["accessToken"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("accessToken not found in credentials"))?;
        Ok(token.to_string())
    }

    fn account_label(&self) -> String {
        let tier = self
            .config_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|s| serde_json::from_str::<Value>(&s).ok())
            .and_then(|v| {
                v["oauthAccount"]["organizationType"]
                    .as_str()
                    .map(String::from)
            })
            .map(|t| match t.as_str() {
                "claude_pro" => "Pro",
                "claude_max" => "Max",
                "team" => "Team",
                _ => "OAuth",
            })
            .unwrap_or("OAuth");

        format!("Claude ({})", tier)
    }
}

#[async_trait]
impl Agent for ClaudeOAuthAgent {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn initial_state(&self) -> SourceState {
        SourceState::Quota(ProviderState {
            provider: Provider::Claude,
            label: self.account_label(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        })
    }

    async fn fetch(&self) -> anyhow::Result<SourceState> {
        let token = self.read_access_token()?;
        let resp: Value = self
            .client
            .get(CLAUDE_OAUTH_USAGE_URL)
            .bearer_auth(&token)
            .header("Content-Type", "application/json")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let windows = parse_oauth_windows(&resp);

        Ok(SourceState::Quota(ProviderState {
            provider: Provider::Claude,
            label: self.account_label(),
            windows,
            last_updated: Some(Utc::now()),
            last_error: None,
        }))
    }
}

fn parse_oauth_windows(resp: &Value) -> Vec<LimitWindow> {
    let mut windows = Vec::new();

    if let Some(util) = resp["five_hour"]["utilization"].as_f64() {
        let reset_at = resp["five_hour"]["resets_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        windows.push(LimitWindow::from_fraction(
            WindowKind::FiveHours,
            None,
            (util / 100.0) as f32,
            reset_at,
        ));
    }

    if let Some(util) = resp["seven_day"]["utilization"].as_f64() {
        let reset_at = resp["seven_day"]["resets_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        windows.push(LimitWindow::from_fraction(
            WindowKind::SevenDays,
            None,
            (util / 100.0) as f32,
            reset_at,
        ));
    }

    windows
}

// ===== API Key Agent (Console) =====

pub struct ClaudeApiAgent {
    source_id: String,
    api_key: String,
    client: reqwest::Client,
}

impl ClaudeApiAgent {
    pub fn from_env() -> Option<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())?;
        let client = reqwest::Client::builder()
            .timeout(http_timeout())
            .build()
            .ok()?;
        Some(Self {
            source_id: "api-key".into(),
            api_key,
            client,
        })
    }
}

#[async_trait]
impl Agent for ClaudeApiAgent {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn source_id(&self) -> &str {
        &self.source_id
    }

    fn initial_state(&self) -> SourceState {
        SourceState::Ceiling(CeilingReport {
            label: "Claude (API)".into(),
            ceilings: vec![],
            last_updated: None,
            last_error: None,
        })
    }

    async fn fetch(&self) -> anyhow::Result<SourceState> {
        let resp: Value = self
            .client
            .get(CLAUDE_API_RATE_LIMITS_URL)
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", CLAUDE_API_VERSION)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;

        let data = resp["data"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing data array in rate_limits response"))?;

        let ceilings = parse_ceilings(data);

        Ok(SourceState::Ceiling(CeilingReport {
            label: "Claude (API)".into(),
            ceilings,
            last_updated: Some(Utc::now()),
            last_error: None,
        }))
    }
}

fn simplify_model_name(name: &str) -> String {
    if let Some(idx) = name.rfind('-') {
        let last = &name[idx + 1..];
        if last.len() == 8 && last.chars().all(|c| c.is_ascii_digit()) {
            return name[..idx].to_string();
        }
    }
    name.to_string()
}

fn parse_ceilings(data: &[Value]) -> Vec<Ceiling> {
    let mut ceilings = Vec::new();
    for entry in data {
        if entry["group_type"].as_str() != Some("model_group") {
            continue;
        }
        let group = entry["models"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|m| m.as_str())
            .map(simplify_model_name)
            .unwrap_or_else(|| "unknown".into());

        let mut rpm = 0;
        let mut in_tpm = 0;
        let mut out_tpm = 0;
        if let Some(limits) = entry["limits"].as_array() {
            for limit in limits {
                let val = limit["value"].as_u64().unwrap_or(0);
                match limit["type"].as_str() {
                    Some("requests_per_minute") => rpm = val,
                    Some("input_tokens_per_minute") => in_tpm = val,
                    Some("output_tokens_per_minute") => out_tpm = val,
                    _ => {}
                }
            }
        }
        ceilings.push(Ceiling {
            group,
            rpm,
            in_tpm,
            out_tpm,
        });
    }
    ceilings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn simplify_model_name_strips_date_suffix() {
        assert_eq!(
            simplify_model_name("claude-sonnet-4-20250514"),
            "claude-sonnet-4"
        );
    }

    #[test]
    fn simplify_model_name_keeps_names_without_date_suffix() {
        assert_eq!(simplify_model_name("claude-sonnet-4"), "claude-sonnet-4");
        assert_eq!(simplify_model_name("claude-haiku-4-5"), "claude-haiku-4-5");
    }

    #[test]
    fn parse_oauth_windows_reads_both_windows() {
        let resp = json!({
            "five_hour": { "utilization": 58.0, "resets_at": "2026-08-13T17:30:00Z" },
            "seven_day": { "utilization": 100.0, "resets_at": "2026-08-20T00:00:00Z" }
        });
        let windows = parse_oauth_windows(&resp);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[0].used, 580);
        assert_eq!(windows[0].limit, crate::config::NOTIONAL_LIMIT);
        assert!(windows[0].reset_at.is_some());
        assert_eq!(windows[1].kind, WindowKind::SevenDays);
        assert_eq!(windows[1].used, 1000);
    }

    #[test]
    fn parse_oauth_windows_handles_missing_fields() {
        let windows = parse_oauth_windows(&json!({}));
        assert!(windows.is_empty());
    }

    #[test]
    fn parse_ceilings_extracts_model_groups_only() {
        let data = json!([
            {
                "group_type": "model_group",
                "models": ["claude-sonnet-4-20250514"],
                "limits": [
                    { "type": "requests_per_minute", "value": 50 },
                    { "type": "input_tokens_per_minute", "value": 100000 },
                    { "type": "output_tokens_per_minute", "value": 20000 }
                ]
            },
            {
                "group_type": "other",
                "models": ["ignored"],
                "limits": []
            }
        ]);
        let ceilings = parse_ceilings(data.as_array().unwrap());
        assert_eq!(ceilings.len(), 1);
        assert_eq!(ceilings[0].group, "claude-sonnet-4");
        assert_eq!(ceilings[0].rpm, 50);
        assert_eq!(ceilings[0].in_tpm, 100000);
        assert_eq!(ceilings[0].out_tpm, 20000);
    }
}
