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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ChatOutput {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thought: Option<String>,
    pub raw: serde_json::Value,
}

#[async_trait]
pub trait LLMClient {
    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatOutput>;
}
