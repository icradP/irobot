use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatOutput {
    pub text: String,
    pub raw: serde_json::Value,
}

#[async_trait]
pub trait LLMClient {
    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatOutput>;
}
