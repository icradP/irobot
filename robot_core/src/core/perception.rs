use crate::utils::InputEvent;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerceptionData {
    pub sentiment: String,
    pub urgency: String,
    pub context_summary: String,
}

#[async_trait]
pub trait PerceptionModule: Send + Sync {
    async fn perceive(&self, input: &InputEvent) -> anyhow::Result<PerceptionData>;
}

pub struct BasicPerceptionModule;

#[async_trait]
impl PerceptionModule for BasicPerceptionModule {
    async fn perceive(&self, _input: &InputEvent) -> anyhow::Result<PerceptionData> {
        // Placeholder: In a real system, this would analyze the input text/event
        Ok(PerceptionData {
            sentiment: "neutral".to_string(),
            urgency: "normal".to_string(),
            context_summary: "No deep analysis".to_string(),
        })
    }
}
