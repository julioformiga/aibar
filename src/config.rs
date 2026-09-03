use crate::model::HyperPlan;
use ratatui::style::Color;
use std::env;
use std::time::Duration;

pub const POLL_INTERVAL_SECS: u64 = 120;
pub const COOLDOWN_SECS: u64 = 30;
pub const HTTP_TIMEOUT_SECS: u64 = 15;
pub const MAX_BACKOFF_SECS: u64 = 1200;
pub const NOTIONAL_LIMIT: u64 = 1000;

pub const CLAUDE_OAUTH_USAGE_URL: &str = "https://api.anthropic.com/api/oauth/usage";
pub const CLAUDE_API_RATE_LIMITS_URL: &str =
    "https://api.anthropic.com/v1/organizations/rate_limits";
pub const CLAUDE_API_VERSION: &str = "2023-06-01";

pub const HYPER_CREDITS_URL: &str = "https://hyper.charm.land/v1/credits";
pub const HYPER_FREE_CREDITS: f64 = 100.0;
pub const HYPER_MONTHLY_CREDITS: f64 = 250.0;

pub const COLOR_LOW_THRESHOLD: f32 = 70.0;
pub const COLOR_HIGH_THRESHOLD: f32 = 90.0;

pub fn poll_interval() -> Duration {
    Duration::from_secs(env_secs("AIBAR_POLL_SECS", POLL_INTERVAL_SECS))
}

pub fn cooldown() -> Duration {
    Duration::from_secs(env_secs("AIBAR_COOLDOWN_SECS", COOLDOWN_SECS))
}

pub fn http_timeout() -> Duration {
    Duration::from_secs(HTTP_TIMEOUT_SECS)
}

pub fn max_backoff() -> Duration {
    Duration::from_secs(MAX_BACKOFF_SECS)
}

/// Mouse capture is on by default; set `AIBAR_NO_MOUSE=1` to keep the
/// terminal's native text selection instead of routing events to the TUI.
pub fn mouse_enabled() -> bool {
    !env::var("AIBAR_NO_MOUSE")
        .map(|v| {
            let v = v.trim().to_ascii_lowercase();
            !(v.is_empty() || v == "0" || v == "false" || v == "no")
        })
        .unwrap_or(false)
}

fn env_secs(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Override manual do plano do Hyper (`AIBAR_HYPER_PLAN=free|monthly`).
/// Ausente ou inválido ⇒ `None` (auto-detecção pelo saldo; ver
/// `CreditsState::resolved_plan`).
pub fn hyper_plan_override() -> Option<HyperPlan> {
    let value = env::var("AIBAR_HYPER_PLAN").ok()?;
    match value.trim().to_ascii_lowercase().as_str() {
        "free" => Some(HyperPlan::Free),
        "monthly" => Some(HyperPlan::Monthly),
        _ => None,
    }
}

pub fn color_for_percentage(pct: f32) -> Color {
    if pct <= COLOR_LOW_THRESHOLD {
        Color::Green
    } else if pct <= COLOR_HIGH_THRESHOLD {
        Color::Yellow
    } else {
        Color::Red
    }
}

/// Serializes tests that mutate process-wide `AIBAR_*` env vars, since
/// `cargo test` runs test functions (including across other modules) on
/// concurrent threads within the same process.
#[cfg(test)]
pub(crate) fn env_var_test_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_thresholds() {
        assert_eq!(color_for_percentage(0.0), Color::Green);
        assert_eq!(color_for_percentage(70.0), Color::Green);
        assert_eq!(color_for_percentage(70.1), Color::Yellow);
        assert_eq!(color_for_percentage(90.0), Color::Yellow);
        assert_eq!(color_for_percentage(90.1), Color::Red);
        assert_eq!(color_for_percentage(100.0), Color::Red);
    }

    #[test]
    fn http_timeout_and_max_backoff_are_fixed() {
        assert_eq!(http_timeout(), Duration::from_secs(HTTP_TIMEOUT_SECS));
        assert_eq!(max_backoff(), Duration::from_secs(MAX_BACKOFF_SECS));
    }

    #[test]
    fn mouse_enabled_defaults_on_and_honors_kill_switch() {
        let _guard = env_var_test_lock().lock().unwrap();

        env::remove_var("AIBAR_NO_MOUSE");
        assert!(mouse_enabled());

        for off in ["1", "true", "yes", "any-value"] {
            env::set_var("AIBAR_NO_MOUSE", off);
            assert!(!mouse_enabled(), "{off} should disable mouse");
        }

        for on in ["", "0", "false", "no"] {
            env::set_var("AIBAR_NO_MOUSE", on);
            assert!(mouse_enabled(), "{on} should keep mouse on");
        }

        env::remove_var("AIBAR_NO_MOUSE");
    }

    #[test]
    fn hyper_plan_override_parses_values_and_ignores_garbage() {
        let _guard = env_var_test_lock().lock().unwrap();

        std::env::remove_var("AIBAR_HYPER_PLAN");
        assert_eq!(hyper_plan_override(), None);

        std::env::set_var("AIBAR_HYPER_PLAN", " Monthly ");
        assert_eq!(hyper_plan_override(), Some(HyperPlan::Monthly));

        std::env::set_var("AIBAR_HYPER_PLAN", "free");
        assert_eq!(hyper_plan_override(), Some(HyperPlan::Free));

        std::env::set_var("AIBAR_HYPER_PLAN", "not-a-plan");
        assert_eq!(hyper_plan_override(), None);

        std::env::remove_var("AIBAR_HYPER_PLAN");
    }

    #[test]
    fn poll_interval_and_cooldown_defaults_and_overrides() {
        let _guard = env_var_test_lock().lock().unwrap();

        env::remove_var("AIBAR_POLL_SECS");
        env::remove_var("AIBAR_COOLDOWN_SECS");
        assert_eq!(poll_interval(), Duration::from_secs(POLL_INTERVAL_SECS));
        assert_eq!(cooldown(), Duration::from_secs(COOLDOWN_SECS));

        env::set_var("AIBAR_POLL_SECS", "42");
        env::set_var("AIBAR_COOLDOWN_SECS", "7");
        assert_eq!(poll_interval(), Duration::from_secs(42));
        assert_eq!(cooldown(), Duration::from_secs(7));

        env::set_var("AIBAR_POLL_SECS", "not-a-number");
        assert_eq!(poll_interval(), Duration::from_secs(POLL_INTERVAL_SECS));

        env::remove_var("AIBAR_POLL_SECS");
        env::remove_var("AIBAR_COOLDOWN_SECS");
    }
}
