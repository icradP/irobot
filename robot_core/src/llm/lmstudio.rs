use crate::llm::adapter::{ChatOutput, ChatRequest, LLMClient};
use async_trait::async_trait;
use bytes::Bytes;
use hyper::body::Incoming;
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use http_body_util::{BodyExt, Full};
use serde_json::json;
use tracing::info;
use url::Url;

#[derive(Clone)]
pub struct LMStudioClient {
    pub base_url: Url,
    pub api_key: Option<String>,
}

impl LMStudioClient {
    pub fn new(base_url: Url, api_key: Option<String>) -> Self {
        Self { base_url, api_key }
    }
}

#[async_trait]
impl LLMClient for LMStudioClient {
    async fn chat(&self, req: ChatRequest) -> anyhow::Result<ChatOutput> {
        let mut endpoint = self.base_url.clone();
        endpoint.set_path("/v1/chat/completions");
        let payload = json!({
            "model": req.model,
            "messages": req.messages,
            "temperature": req.temperature.unwrap_or(0.7),
        });
        let connector = HttpConnector::new();
        let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(endpoint.as_str())
            .header("content-type", "application/json");
        if let Some(key) = &self.api_key {
            builder = builder.header("authorization", format!("Bearer {}", key));
        }
        let req = builder.body(Full::new(Bytes::from(payload.to_string())))?;
        info!("lmstudio request {}", endpoint);
        let res: hyper::Response<Incoming> = client.request(req).await?;
        let status = res.status();
        let body_bytes = res.into_body().collect().await?.to_bytes();
        let raw: serde_json::Value = serde_json::from_slice(&body_bytes)?;
        if !status.is_success() {
            return Err(anyhow::anyhow!(format!("status {} error: {}", status, raw)));
        }
        let text = raw["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        Ok(ChatOutput { text, raw })
    }
}
