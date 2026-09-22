use crate::agents::Agent;
use crate::config::http_timeout;
use crate::model::{LimitWindow, Provider, ProviderState, SourceState, WindowKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::process::Command;

const USAGES_URL: &str = "https://api.kimi.com/coding/v1/usages";

pub struct KimiAgent {
    key: String,
    client: reqwest::Client,
}

impl KimiAgent {
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("KIMI_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(read_key_from_pass)?;
        let client = reqwest::Client::builder()
            .timeout(http_timeout())
            .build()
            .ok()?;
        Some(Self { key, client })
    }
}

fn read_key_from_pass() -> Option<String> {
    let output = Command::new("pass").arg("KIMI_API_KEY").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let key = String::from_utf8(output.stdout).ok()?;
    let trimmed = key.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[async_trait]
impl Agent for KimiAgent {
    fn provider(&self) -> Provider {
        Provider::Kimi
    }

    fn source_id(&self) -> &str {
        "default"
    }

    fn initial_state(&self) -> SourceState {
        SourceState::Quota(ProviderState {
            provider: Provider::Kimi,
            label: "Kimi Code".into(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        })
    }

    async fn fetch(&self) -> anyhow::Result<SourceState> {
        let resp = self
            .client
            .get(USAGES_URL)
            .bearer_auth(&self.key)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;

        Ok(SourceState::Quota(ProviderState {
            provider: Provider::Kimi,
            label: "Kimi Code".into(),
            windows: parse_usages(&resp),
            last_updated: Some(Utc::now()),
            last_error: None,
        }))
    }
}

/// Numeric fields arrive as strings in `limits[].detail` ("100") and may be
/// plain numbers in `usage` — accept both.
fn value_u64(v: &Value) -> Option<u64> {
    v.as_u64()
        .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
}

fn parse_reset(v: &Value) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(v.as_str()?)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// `window.duration` + `window.timeUnit` ("TIME_UNIT_MINUTE"/"HOUR"/"DAY")
/// normalized to minutes, then mapped onto the canonical kinds like Codex:
/// 300 ⇒ 5h, 10080 ⇒ 7d, anything else stays `Minutes(n)`.
fn parse_window_kind(window: &Value) -> Option<WindowKind> {
    let duration = value_u64(&window["duration"])? as u32;
    let unit = window["timeUnit"].as_str()?;
    let minutes = match unit {
        "TIME_UNIT_MINUTE" => duration,
        "TIME_UNIT_HOUR" => duration.checked_mul(60)?,
        "TIME_UNIT_DAY" => duration.checked_mul(1440)?,
        _ => return None,
    };
    Some(match minutes {
        300 => WindowKind::FiveHours,
        10080 => WindowKind::SevenDays,
        n => WindowKind::Minutes(n),
    })
}

fn window_from_detail(kind: WindowKind, detail: &Value) -> Option<LimitWindow> {
    let limit = value_u64(&detail["limit"])?;
    if limit == 0 {
        return None;
    }
    let used = value_u64(&detail["used"])
        .or_else(|| value_u64(&detail["remaining"]).map(|r| limit.saturating_sub(r)))?;
    Some(LimitWindow::from_values(
        kind,
        None,
        used,
        limit,
        parse_reset(&detail["resetTime"]),
    ))
}

/// Maps the `/usages` response into bars: one per `limits[]` rolling window
/// (verified shape: a single 300-minute window), plus the weekly quota in
/// `usage` (present only on some plans) as a 7d window and the monthly quota
/// in `usages.limit_month_total` (ratio only, no absolute counts) as a 1M
/// window.
fn parse_usages(resp: &Value) -> Vec<LimitWindow> {
    let mut windows = Vec::new();

    if let Some(limits) = resp["limits"].as_array() {
        for limit in limits {
            let Some(kind) = parse_window_kind(&limit["window"]) else {
                continue;
            };
            if let Some(w) = window_from_detail(kind, &limit["detail"]) {
                windows.push(w);
            }
        }
    }

    let usage = &resp["usage"];
    if let Some(limit) = value_u64(&usage["limit"]) {
        if limit > 0 {
            let used = value_u64(&usage["used"])
                .or_else(|| value_u64(&usage["remaining"]).map(|r| limit.saturating_sub(r)))
                .unwrap_or(0);
            windows.push(LimitWindow::from_values(
                WindowKind::SevenDays,
                None,
                used,
                limit,
                parse_reset(&usage["resetTime"]),
            ));
        }
    }

    // `usages.limit_month_total` reports only a `used_ratio` (0..1) and a
    // snake_case `reset_time`; scale the ratio onto the notional limit.
    let month = &resp["usages"]["limit_month_total"];
    if let Some(ratio) = month["used_ratio"].as_f64() {
        windows.push(LimitWindow::from_fraction(
            WindowKind::Month,
            None,
            ratio as f32,
            parse_reset(&month["reset_time"]),
        ));
    }

    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_usages_handles_verified_rolling_window_shape() {
        // Shape captured from a live `GET /coding/v1/usages` call: string
        // numerics, 300-minute rolling window, no weekly `usage` block.
        let resp = json!({
            "limits": [{
                "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": {
                    "limit": "100",
                    "used": "68",
                    "remaining": "32",
                    "resetTime": "2026-09-15T11:44:53.756331Z"
                }
            }]
        });
        let windows = parse_usages(&resp);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[0].used, 68);
        assert_eq!(windows[0].limit, 100);
        assert!(windows[0].reset_at.is_some());
    }

    #[test]
    fn parse_usages_includes_monthly_total_ratio() {
        // Shape captured from a live call: `usages` carries ratio-only quotas
        // (`used_ratio` + snake_case `reset_time`); only `limit_month_total`
        // becomes a bar — `limit_5h` duplicates `limits[]` and
        // `limit_month_code` is not shown.
        let resp = json!({
            "limits": [{
                "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": { "limit": "100", "used": "6", "resetTime": "2026-09-22T13:44:53Z" }
            }],
            "usages": {
                "limit_5h": { "used_ratio": 0.06, "reset_time": "2026-09-22T13:44:53Z" },
                "limit_month_total": { "used_ratio": 0.4244, "reset_time": "2026-10-16T00:00:00Z" },
                "limit_month_code": { "used_ratio": 0.0, "reset_time": "2026-10-16T00:00:00Z" }
            }
        });
        let windows = parse_usages(&resp);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[1].kind, WindowKind::Month);
        assert_eq!(windows[1].used, 424);
        assert_eq!(windows[1].limit, crate::config::NOTIONAL_LIMIT);
        assert_eq!(
            windows[1].reset_at,
            Some(DateTime::parse_from_rfc3339("2026-10-16T00:00:00Z").unwrap().with_timezone(&Utc))
        );
    }

    #[test]
    fn parse_usages_includes_weekly_quota_when_present() {
        let resp = json!({
            "limits": [{
                "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": { "limit": "100", "remaining": "32", "resetTime": "2026-09-15T11:44:53Z" }
            }],
            "usage": { "limit": 500, "remaining": 120, "resetTime": "2026-09-22T00:00:00Z" }
        });
        let windows = parse_usages(&resp);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[0].used, 68);
        assert_eq!(windows[1].kind, WindowKind::SevenDays);
        assert_eq!(windows[1].used, 380);
        assert_eq!(windows[1].limit, 500);
        assert!(windows[1].reset_at.is_some());
    }

    #[test]
    fn parse_usages_maps_other_durations_to_minutes() {
        let resp = json!({
            "limits": [{
                "window": { "duration": 90, "timeUnit": "TIME_UNIT_MINUTE" },
                "detail": { "limit": "10", "used": "1" }
            }]
        });
        let windows = parse_usages(&resp);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, WindowKind::Minutes(90));
    }

    #[test]
    fn parse_usages_normalizes_hour_and_day_units() {
        let resp = json!({
            "limits": [
                {
                    "window": { "duration": 5, "timeUnit": "TIME_UNIT_HOUR" },
                    "detail": { "limit": "10", "used": "1" }
                },
                {
                    "window": { "duration": 7, "timeUnit": "TIME_UNIT_DAY" },
                    "detail": { "limit": "20", "used": "2" }
                }
            ]
        });
        let windows = parse_usages(&resp);
        assert_eq!(windows.len(), 2);
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[1].kind, WindowKind::SevenDays);
    }

    #[test]
    fn parse_usages_skips_unknown_units_zero_limits_and_garbage() {
        let resp = json!({
            "limits": [
                {
                    "window": { "duration": 5, "timeUnit": "TIME_UNIT_SECOND" },
                    "detail": { "limit": "10", "used": "1" }
                },
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "0", "used": "0" }
                },
                {
                    "window": { "duration": 300, "timeUnit": "TIME_UNIT_MINUTE" },
                    "detail": { "limit": "abc" }
                }
            ]
        });
        assert!(parse_usages(&resp).is_empty());
        assert!(parse_usages(&json!({})).is_empty());
    }

    #[test]
    fn value_u64_accepts_strings_and_numbers() {
        assert_eq!(value_u64(&json!("100")), Some(100));
        assert_eq!(value_u64(&json!(42)), Some(42));
        assert_eq!(value_u64(&json!("nope")), None);
        assert_eq!(value_u64(&Value::Null), None);
    }
}
