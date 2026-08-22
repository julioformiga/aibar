pub mod claude;
pub mod gemini;
pub mod hyper;
pub mod zai;

use crate::model::{Provider, SourceState};
use async_trait::async_trait;

#[async_trait]
pub trait Agent: Send + Sync {
    fn provider(&self) -> Provider;
    fn source_id(&self) -> &str;
    fn initial_state(&self) -> SourceState;
    async fn fetch(&self) -> anyhow::Result<SourceState>;
}

pub fn detect_agents() -> Vec<Box<dyn Agent>> {
    let mut agents: Vec<Box<dyn Agent>> = Vec::new();

    for oauth in claude::ClaudeOAuthAgent::detect_all() {
        tracing::info!(source_id = oauth.source_id(), "detected claude oauth");
        agents.push(Box::new(oauth));
    }
    match claude::ClaudeApiAgent::from_env() {
        Some(a) => {
            tracing::info!("detected claude api key");
            agents.push(Box::new(a));
        }
        None => tracing::info!("claude api key not detected"),
    }
    match zai::ZaiAgent::from_env() {
        Some(a) => {
            tracing::info!("detected z.ai");
            agents.push(Box::new(a));
        }
        None => tracing::info!("z.ai not detected"),
    }
    match gemini::GeminiAgent::from_env() {
        Some(a) => {
            tracing::info!("detected gemini");
            agents.push(Box::new(a));
        }
        None => tracing::info!("gemini not detected"),
    }
    match hyper::HyperAgent::from_env() {
        Some(a) => {
            tracing::info!("detected hyper");
            agents.push(Box::new(a));
        }
        None => tracing::info!("hyper not detected"),
    }
    agents
}
