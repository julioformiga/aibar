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
    Hyper,
}

impl Provider {
    pub fn label(self) -> &'static str {
        match self {
            Provider::Claude => "Claude",
            Provider::Zai => "Z.ai",
            Provider::Gemini => "Gemini",
            Provider::Hyper => "Hyper",
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

/// Plano do Hyper que lastreia a barra de créditos: o `/v1/credits` não
/// informa o plano, então ele é inferido do saldo e "lembrado" entre polls
/// (ver `CreditsState::resolved_plan`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HyperPlan {
    /// Gratuito: 100 Hypercredits/mês (`HYPER_FREE_CREDITS`).
    Free,
    /// Assinatura: 250 Hypercredits/dia (`HYPER_MONTHLY_CREDITS`).
    Monthly,
}

/// Snapshot de saldo de créditos pré-pagos (ex.: Hypercredits do Hyper).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreditsState {
    pub label: String,
    pub balance: Option<f64>,
    /// Plano detectado (sticky entre polls; ver `resolved_plan`).
    #[serde(default)]
    pub plan: Option<HyperPlan>,
    /// Melhor estimativa do próximo refresh do plano, ancorada na última
    /// vez que o saldo subiu entre dois polls.
    #[serde(default)]
    pub reset_at: Option<DateTime<Utc>>,
    pub last_updated: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

impl CreditsState {
    /// Plano efetivo: override `AIBAR_HYPER_PLAN` > plano lembrado >
    /// heurística pelo saldo (saldo acima da mesada gratuita só existe no
    /// plano mensal).
    pub fn resolved_plan(&self) -> HyperPlan {
        if let Some(plan) = crate::config::hyper_plan_override() {
            return plan;
        }
        if let Some(plan) = self.plan {
            return plan;
        }
        match self.balance {
            Some(b) if b > crate::config::HYPER_FREE_CREDITS => HyperPlan::Monthly,
            _ => HyperPlan::Free,
        }
    }

    /// Mesada do plano resolvido, usada como denominador da barra.
    pub fn allowance(&self) -> f64 {
        match self.resolved_plan() {
            HyperPlan::Free => crate::config::HYPER_FREE_CREDITS,
            HyperPlan::Monthly => crate::config::HYPER_MONTHLY_CREDITS,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SourceState {
    Quota(ProviderState),
    Ceiling(CeilingReport),
    Credits(CreditsState),
}

impl SourceState {
    pub fn label(&self) -> &str {
        match self {
            SourceState::Quota(p) => &p.label,
            SourceState::Ceiling(c) => &c.label,
            SourceState::Credits(c) => &c.label,
        }
    }

    pub fn last_error(&self) -> Option<&str> {
        match self {
            SourceState::Quota(p) => p.last_error.as_deref(),
            SourceState::Ceiling(c) => c.last_error.as_deref(),
            SourceState::Credits(c) => c.last_error.as_deref(),
        }
    }

    #[allow(dead_code)]
    pub fn last_updated(&self) -> Option<DateTime<Utc>> {
        match self {
            SourceState::Quota(p) => p.last_updated,
            SourceState::Ceiling(c) => c.last_updated,
            SourceState::Credits(c) => c.last_updated,
        }
    }

    pub fn set_error(&mut self, err: String) {
        match self {
            SourceState::Quota(p) => p.last_error = Some(err),
            SourceState::Ceiling(c) => c.last_error = Some(err),
            SourceState::Credits(c) => c.last_error = Some(err),
        }
    }

    /// Returns true when the observable usage of this source changed relative
    /// to `other`. `Quota` compares every window (`used`/`limit` per kind and
    /// scope, so a change in any window counts, not just the max); `Credits`
    /// compares the raw balance; `Ceiling` never reports a change (it has no
    /// percentage). A side with no data yet (`Quota` with no windows, or no
    /// balance) is treated as "no change" so the first poll never fires.
    pub fn usage_changed(&self, other: &SourceState) -> bool {
        match (self, other) {
            (SourceState::Quota(a), SourceState::Quota(b)) => {
                if a.windows.is_empty() || b.windows.is_empty() {
                    return false;
                }
                let signature = |p: &ProviderState| {
                    let mut v: Vec<(u8, Option<u8>, u64, u64)> = p
                        .windows
                        .iter()
                        .map(|w| {
                            let kind = match w.kind {
                                WindowKind::FiveHours => 0u8,
                                WindowKind::SevenDays => 1u8,
                            };
                            let scope = match w.scope {
                                Some(LimitScope::Standard) => Some(0u8),
                                Some(LimitScope::ThirdParty) => Some(1u8),
                                None => None,
                            };
                            (kind, scope, w.used, w.limit)
                        })
                        .collect();
                    v.sort_unstable();
                    v
                };
                signature(a) != signature(b)
            }
            (SourceState::Credits(a), SourceState::Credits(b)) => match (a.balance, b.balance) {
                (Some(x), Some(y)) => x != y,
                _ => false,
            },
            _ => false,
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

    fn quota_with(used_5h: u64, used_7d: u64) -> SourceState {
        SourceState::Quota(ProviderState {
            provider: Provider::Claude,
            label: "Claude".into(),
            windows: vec![
                LimitWindow::from_values(
                    WindowKind::FiveHours,
                    None,
                    used_5h,
                    crate::config::NOTIONAL_LIMIT,
                    None,
                ),
                LimitWindow::from_values(
                    WindowKind::SevenDays,
                    None,
                    used_7d,
                    crate::config::NOTIONAL_LIMIT,
                    None,
                ),
            ],
            last_updated: None,
            last_error: None,
        })
    }

    #[test]
    fn usage_changed_detects_change_in_any_window_not_just_max() {
        // 7d (bigger) unchanged, 5h changes: must still count as changed.
        let before = quota_with(100, 700);
        let after = quota_with(150, 700);
        assert!(before.usage_changed(&after));
    }

    #[test]
    fn usage_changed_is_false_when_windows_identical() {
        let a = quota_with(100, 700);
        let b = quota_with(100, 700);
        assert!(!a.usage_changed(&b));
    }

    #[test]
    fn usage_changed_ignores_first_poll_with_no_prior_data() {
        let empty = SourceState::Quota(ProviderState {
            provider: Provider::Claude,
            label: "Claude".into(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        });
        let filled = quota_with(100, 700);
        assert!(!empty.usage_changed(&filled));
        assert!(!filled.usage_changed(&empty));
    }

    #[test]
    fn usage_changed_compares_credits_balance() {
        let credits = |b: Option<f64>| {
            SourceState::Credits(CreditsState {
                label: "Hyper".into(),
                balance: b,
                plan: None,
                reset_at: None,
                last_updated: None,
                last_error: None,
            })
        };
        assert!(credits(Some(90.0)).usage_changed(&credits(Some(80.0))));
        assert!(!credits(Some(90.0)).usage_changed(&credits(Some(90.0))));
        assert!(!credits(None).usage_changed(&credits(Some(90.0))));
    }

    fn credits_state(
        balance: Option<f64>,
        plan: Option<HyperPlan>,
        reset_at: Option<DateTime<Utc>>,
    ) -> CreditsState {
        CreditsState {
            label: "Hyper".into(),
            balance,
            plan,
            reset_at,
            last_updated: None,
            last_error: None,
        }
    }

    #[test]
    fn resolved_plan_prefers_env_override() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();

        std::env::set_var("AIBAR_HYPER_PLAN", "monthly");
        assert_eq!(
            credits_state(Some(30.0), None, None).resolved_plan(),
            HyperPlan::Monthly
        );

        std::env::set_var("AIBAR_HYPER_PLAN", "free");
        assert_eq!(
            credits_state(Some(250.0), Some(HyperPlan::Monthly), None).resolved_plan(),
            HyperPlan::Free
        );

        std::env::set_var("AIBAR_HYPER_PLAN", "not-a-plan");
        assert_eq!(
            credits_state(Some(30.0), None, None).resolved_plan(),
            HyperPlan::Free
        );

        std::env::remove_var("AIBAR_HYPER_PLAN");
    }

    #[test]
    fn resolved_plan_sticky_then_balance_heuristic() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");

        // Lembrou mensal: saldo baixo no fim do dia continua mensal.
        assert_eq!(
            credits_state(Some(30.0), Some(HyperPlan::Monthly), None).resolved_plan(),
            HyperPlan::Monthly
        );

        // Sem plano lembrado, heurística pelo saldo.
        assert_eq!(
            credits_state(Some(109.0), None, None).resolved_plan(),
            HyperPlan::Monthly
        );
        assert_eq!(
            credits_state(Some(100.0), None, None).resolved_plan(),
            HyperPlan::Free
        );
        assert_eq!(
            credits_state(None, None, None).resolved_plan(),
            HyperPlan::Free
        );
    }

    #[test]
    fn allowance_matches_resolved_plan() {
        let _guard = crate::config::env_var_test_lock().lock().unwrap();
        std::env::remove_var("AIBAR_HYPER_PLAN");

        assert_eq!(credits_state(Some(30.0), None, None).allowance(), 100.0);
        assert_eq!(
            credits_state(Some(250.0), Some(HyperPlan::Free), None).allowance(),
            100.0
        );
        assert_eq!(
            credits_state(Some(30.0), Some(HyperPlan::Monthly), None).allowance(),
            250.0
        );
    }

    #[test]
    fn usage_changed_never_fires_for_ceiling() {
        let ceiling = |label: &str| {
            SourceState::Ceiling(CeilingReport {
                label: label.into(),
                ceilings: vec![],
                last_updated: None,
                last_error: None,
            })
        };
        assert!(!ceiling("a").usage_changed(&ceiling("b")));
    }
}
