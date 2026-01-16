use crate::llm::adapter::{ChatOutput, ChatRequest, LLMClient};
use async_trait::async_trait;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
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
        let request = builder.body(Full::new(Bytes::from(payload.to_string())))?;
        info!("lmstudio request {}", endpoint);
        let res: hyper::Response<Incoming> = client.request(request).await?;
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
        
        let (mut clean_text, thought) = remove_think_tags(&text);
        if clean_text.trim().is_empty() && !text.trim().is_empty() {
            clean_text = text;
        }

        if let Some(thought_content) = &thought {
            if let Some(sid) = &req.session_id {
                let evt = crate::utils::OutputEvent {
                    target: "default".into(),
                    source: "llm".into(),
                    session_id: Some(sid.clone()),
                    content: serde_json::json!({
                        "type": "think",
                        "content": thought_content
                    }),
                    style: "neutral".to_string(),
                };
                 let _ = crate::utils::output_bus().send(evt);
            }
        }

        Ok(ChatOutput { text: clean_text, thought, raw })
    }
}

fn remove_think_tags(text: &str) -> (String, Option<String>) {
    let mut result = String::new();
    let mut thought = String::new();
    let mut remaining = text;
    let mut has_thought = false;
    
    while let Some(start_idx) = remaining.find("<think>") {
        result.push_str(&remaining[..start_idx]);
        if let Some(end_idx) = remaining[start_idx..].find("</think>") {
             let t_slice = &remaining[start_idx..][7..end_idx];
             thought.push_str(t_slice);
             has_thought = true;
             remaining = &remaining[start_idx + end_idx + 8..];
        } else {
             if remaining.len() > start_idx + 7 {
                thought.push_str(&remaining[start_idx+7..]);
                has_thought = true;
             }
             remaining = "";
             break;
        }
    }
    result.push_str(remaining);
    (result, if has_thought { Some(thought) } else { None })
}
