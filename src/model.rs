use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum WindowKind {
    FiveHours,
    SevenDays,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LimitScope {
    Standard,
    ThirdParty,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LimitWindow {
    pub kind: WindowKind,
    pub scope: Option<LimitScope>,
    pub used: u64,
    pub limit: u64,
    pub reset_at: Option<DateTime<Utc>>,
}

impl LimitWindow {
    pub fn percentage(&self) -> f32 {
        if self.limit == 0 {
            0.0
        } else {
            (self.used as f32 / self.limit as f32) * 100.0
        }
    }

    #[allow(dead_code)]
    pub fn remaining(&self) -> u64 {
        self.limit.saturating_sub(self.used)
    }

    pub fn from_fraction(
        kind: WindowKind,
        scope: Option<LimitScope>,
        used_fraction: f32,
        reset_at: Option<DateTime<Utc>>,
    ) -> Self {
        let limit = crate::config::NOTIONAL_LIMIT;
        let used = (used_fraction.clamp(0.0, 1.0) * limit as f32).round() as u64;
        Self {
            kind,
            scope,
            used,
            limit,
            reset_at,
        }
    }

    pub fn from_values(
        kind: WindowKind,
        scope: Option<LimitScope>,
        used: u64,
        limit: u64,
        reset_at: Option<DateTime<Utc>>,
    ) -> Self {
        Self {
            kind,
            scope,
            used,
            limit,
            reset_at,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Provider {
    Claude,
    Zai,
    Gemini,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Zai => "Z.ai",
            Provider::Gemini => "Gemini",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProviderState {
    pub provider: Provider,
    pub label: String,
    pub windows: Vec<LimitWindow>,
    pub last_updated: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Ceiling {
    pub group: String,
    pub rpm: u64,
    pub in_tpm: u64,
    pub out_tpm: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CeilingReport {
    pub label: String,
    pub ceilings: Vec<Ceiling>,
    pub last_updated: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SourceState {
    Quota(ProviderState),
    Ceiling(CeilingReport),
}

impl SourceState {
    pub fn label(&self) -> &str {
        match self {
            SourceState::Quota(p) => &p.label,
            SourceState::Ceiling(c) => &c.label,
        }
    }

    pub fn last_error(&self) -> Option<&str> {
        match self {
            SourceState::Quota(p) => p.last_error.as_deref(),
            SourceState::Ceiling(c) => c.last_error.as_deref(),
        }
    }

    #[allow(dead_code)]
    pub fn last_updated(&self) -> Option<DateTime<Utc>> {
        match self {
            SourceState::Quota(p) => p.last_updated,
            SourceState::Ceiling(c) => c.last_updated,
        }
    }

    pub fn set_error(&mut self, err: String) {
        match self {
            SourceState::Quota(p) => p.last_error = Some(err),
            SourceState::Ceiling(c) => c.last_error = Some(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentage_zero_limit_is_zero() {
        let w = LimitWindow {
            kind: WindowKind::FiveHours,
            scope: None,
            used: 5,
            limit: 0,
            reset_at: None,
        };
        assert_eq!(w.percentage(), 0.0);
    }

    #[test]
    fn percentage_computes_ratio() {
        let w = LimitWindow {
            kind: WindowKind::FiveHours,
            scope: None,
            used: 25,
            limit: 200,
            reset_at: None,
        };
        assert_eq!(w.percentage(), 12.5);
    }

    #[test]
    fn remaining_saturates_at_zero() {
        let w = LimitWindow {
            kind: WindowKind::FiveHours,
            scope: None,
            used: 10,
            limit: 5,
            reset_at: None,
        };
        assert_eq!(w.remaining(), 0);
    }

    #[test]
    fn remaining_computes_difference() {
        let w = LimitWindow {
            kind: WindowKind::FiveHours,
            scope: None,
            used: 30,
            limit: 100,
            reset_at: None,
        };
        assert_eq!(w.remaining(), 70);
    }

    #[test]
    fn from_fraction_clamps_and_scales_to_notional_limit() {
        let w = LimitWindow::from_fraction(WindowKind::SevenDays, None, 1.5, None);
        assert_eq!(w.used, crate::config::NOTIONAL_LIMIT);
        assert_eq!(w.limit, crate::config::NOTIONAL_LIMIT);

        let w = LimitWindow::from_fraction(WindowKind::SevenDays, None, -0.5, None);
        assert_eq!(w.used, 0);

        let w = LimitWindow::from_fraction(WindowKind::SevenDays, None, 0.58, None);
        assert_eq!(w.used, 580);
    }

    #[test]
    fn from_values_passes_through_used_and_limit() {
        let w = LimitWindow::from_values(
            WindowKind::FiveHours,
            Some(LimitScope::Standard),
            12,
            34,
            None,
        );
        assert_eq!(w.used, 12);
        assert_eq!(w.limit, 34);
        assert_eq!(w.scope, Some(LimitScope::Standard));
    }

    #[test]
    fn provider_labels() {
        assert_eq!(Provider::Claude.label(), "Claude");
        assert_eq!(Provider::Zai.label(), "Z.ai");
        assert_eq!(Provider::Gemini.label(), "Gemini");
    }

    #[test]
    fn source_state_label_and_errors() {
        let mut quota = SourceState::Quota(ProviderState {
            provider: Provider::Claude,
            label: "Claude (Pro)".into(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        });
        assert_eq!(quota.label(), "Claude (Pro)");
        assert_eq!(quota.last_error(), None);
        quota.set_error("boom".into());
        assert_eq!(quota.last_error(), Some("boom"));

        let mut ceiling = SourceState::Ceiling(CeilingReport {
            label: "Claude (API)".into(),
            ceilings: vec![],
            last_updated: None,
            last_error: None,
        });
        assert_eq!(ceiling.label(), "Claude (API)");
        assert_eq!(ceiling.last_updated(), None);
        ceiling.set_error("nope".into());
        assert_eq!(ceiling.last_error(), Some("nope"));
    }
}
