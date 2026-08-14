use crate::agents::Agent;
use crate::config::http_timeout;
use crate::model::{LimitWindow, Provider, ProviderState, SourceState, WindowKind};
use async_trait::async_trait;
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use std::process::Command;

pub struct ZaiAgent {
    key: String,
    client: reqwest::Client,
}

impl ZaiAgent {
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("ZAI_API_KEY")
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
    let output = Command::new("pass").arg("Z_AI_API_KEY").output().ok()?;
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
impl Agent for ZaiAgent {
    fn provider(&self) -> Provider {
        Provider::Zai
    }

    fn source_id(&self) -> &str {
        "default"
    }

    fn initial_state(&self) -> SourceState {
        SourceState::Quota(ProviderState {
            provider: Provider::Zai,
            label: "Z.ai".into(),
            windows: vec![],
            last_updated: None,
            last_error: None,
        })
    }

    async fn fetch(&self) -> anyhow::Result<SourceState> {
        let resp = self
            .client
            .get("https://api.z.ai/api/monitor/usage/quota/limit")
            .bearer_auth(&self.key)
            .header("Accept-Language", "en-US,en")
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;

        let limits = resp["data"]["limits"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing data.limits in z.ai response"))?;

        let windows = parse_limits(limits);

        Ok(SourceState::Quota(ProviderState {
            provider: Provider::Zai,
            label: "Z.ai".into(),
            windows,
            last_updated: Some(Utc::now()),
            last_error: None,
        }))
    }
}

#[allow(dead_code)]
fn _parse_reset(_v: &Value) -> Option<DateTime<Utc>> {
    None
}

fn parse_limits(limits: &[Value]) -> Vec<LimitWindow> {
    let mut windows = Vec::new();
    for limit in limits {
        let unit = limit["unit"].as_u64();
        let Some(unit) = unit else { continue };
        let kind = match unit {
            3 => WindowKind::FiveHours,
            6 => WindowKind::SevenDays,
            _ => continue,
        };
        let used = limit["currentValue"].as_u64();
        let total = limit["usage"].as_u64();
        let remaining = limit["remaining"].as_u64();

        let window = if let (Some(used), Some(total)) = (used, total) {
            if total > 0 {
                Some(LimitWindow::from_values(kind, None, used, total, None))
            } else {
                None
            }
        } else if let (Some(used), Some(rem)) = (used, remaining) {
            Some(LimitWindow::from_values(kind, None, used, used + rem, None))
        } else if let Some(pct) = limit["percentage"].as_f64() {
            let reset_at = limit["nextResetTime"]
                .as_u64()
                .and_then(|ms| Utc.timestamp_millis_opt(ms as i64).single());
            Some(LimitWindow::from_fraction(
                kind,
                None,
                (pct / 100.0) as f32,
                reset_at,
            ))
        } else {
            None
        };

        if let Some(mut w) = window {
            if w.reset_at.is_none() {
                w.reset_at = limit["nextResetTime"]
                    .as_u64()
                    .and_then(|ms| Utc.timestamp_millis_opt(ms as i64).single());
            }
            windows.push(w);
        }
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_limits_uses_current_and_usage_when_present() {
        let limits = json!([
            { "unit": 3, "currentValue": 580, "usage": 1000, "nextResetTime": 1_755_112_200_000i64 }
        ]);
        let windows = parse_limits(limits.as_array().unwrap());
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, WindowKind::FiveHours);
        assert_eq!(windows[0].used, 580);
        assert_eq!(windows[0].limit, 1000);
        assert!(windows[0].reset_at.is_some());
    }

    #[test]
    fn parse_limits_falls_back_to_remaining() {
        let limits = json!([
            { "unit": 6, "currentValue": 200, "remaining": 800 }
        ]);
        let windows = parse_limits(limits.as_array().unwrap());
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].kind, WindowKind::SevenDays);
        assert_eq!(windows[0].used, 200);
        assert_eq!(windows[0].limit, 1000);
    }

    #[test]
    fn parse_limits_falls_back_to_percentage() {
        let limits = json!([
            { "unit": 3, "percentage": 42.0 }
        ]);
        let windows = parse_limits(limits.as_array().unwrap());
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].used, 420);
        assert_eq!(windows[0].limit, crate::config::NOTIONAL_LIMIT);
    }

    #[test]
    fn parse_limits_skips_unknown_units_and_empty_totals() {
        let limits = json!([
            { "unit": 99, "currentValue": 1, "usage": 1 },
            { "unit": 3, "currentValue": 1, "usage": 0 }
        ]);
        let windows = parse_limits(limits.as_array().unwrap());
        assert!(windows.is_empty());
    }
}
