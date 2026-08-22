use crate::agents::Agent;
use crate::config::{http_timeout, HYPER_CREDITS_URL};
use crate::model::{CreditsState, Provider, SourceState};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::Value;

pub struct HyperAgent {
    key: String,
    client: reqwest::Client,
}

impl HyperAgent {
    pub fn from_env() -> Option<Self> {
        let key = std::env::var("HYPER_API_KEY")
            .ok()
            .filter(|s| !s.is_empty())?;
        let client = reqwest::Client::builder()
            .timeout(http_timeout())
            .build()
            .ok()?;
        Some(Self { key, client })
    }
}

#[async_trait]
impl Agent for HyperAgent {
    fn provider(&self) -> Provider {
        Provider::Hyper
    }

    fn source_id(&self) -> &str {
        "default"
    }

    fn initial_state(&self) -> SourceState {
        SourceState::Credits(CreditsState {
            label: "Hyper".into(),
            balance: None,
            last_updated: None,
            last_error: None,
        })
    }

    async fn fetch(&self) -> anyhow::Result<SourceState> {
        let resp = self
            .client
            .get(HYPER_CREDITS_URL)
            .bearer_auth(&self.key)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;

        let balance = parse_balance(&resp)
            .ok_or_else(|| anyhow::anyhow!("missing balance in hyper response"))?;

        Ok(SourceState::Credits(CreditsState {
            label: "Hyper".into(),
            balance: Some(balance),
            last_updated: Some(Utc::now()),
            last_error: None,
        }))
    }
}

fn parse_balance(resp: &Value) -> Option<f64> {
    resp["balance"].as_f64()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_balance_reads_integer_and_fractional() {
        assert_eq!(parse_balance(&json!({ "balance": 100 })), Some(100.0));
        assert_eq!(parse_balance(&json!({ "balance": 42.5 })), Some(42.5));
    }

    #[test]
    fn parse_balance_returns_none_when_missing() {
        assert_eq!(parse_balance(&json!({})), None);
        assert_eq!(parse_balance(&json!({ "balance": null })), None);
        assert_eq!(parse_balance(&json!({ "balance": "100" })), None);
    }
}
