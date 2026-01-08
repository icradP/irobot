use schemars::JsonSchema;
use serde::{Serialize, Deserialize};
use rmcp::{ErrorData, model::*, service::{RoleServer, RequestContext}};
use std::sync::Arc;
use crate::tools::{to_object, ToolEntry};
use url::Url;
use hyper::body::Incoming;
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use http_body_util::{BodyExt, Full};
use bytes::Bytes;
// remove unused duplicated imports

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessage { pub role: String, pub content: String }
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChatRequest { pub model: Option<String>, pub messages: Vec<ChatMessage>, pub temperature: Option<f32> }

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(ChatRequest);
    let tool = Tool {
        name: "chat".into(),
        title: Some("Chat".into()),
        description: Some("[Conversational] Engages in open-ended conversation, answers general questions, and handles small talk.".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None, annotations: None, icons: None, meta: None,
    };
    ToolEntry {
        name: "chat",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let args: ChatRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        match context.peer.elicit::<ChatRequest>("请输入聊天的参数".to_string()).await {
            Ok(Some(params)) => params,
            Ok(None) => return Err(ErrorData::invalid_params("未提供参数", None)),
            Err(e) => return Err(ErrorData::internal_error(format!("elicitation 错误: {}", e), None)),
        }
    };
    let base = std::env::var("LMSTUDIO_URL").unwrap_or_else(|_| "http://localhost:1234".to_string());
    let api_key = std::env::var("LMSTUDIO_API_KEY").ok();
    let model = args.model.unwrap_or_else(|| std::env::var("LMSTUDIO_MODEL").unwrap_or_else(|_| "default".to_string()));
    let mut endpoint = Url::parse(&base).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    endpoint.set_path("/v1/chat/completions");
    let payload = serde_json::json!({
        "model": model,
        "messages": args.messages,
        "temperature": args.temperature.unwrap_or(0.2),
    });
    let connector = HttpConnector::new();
    let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);
    let mut builder = Request::builder()
        .method(Method::POST)
        .uri(endpoint.as_str())
        .header("content-type", "application/json");
    if let Some(key) = api_key {
        builder = builder.header("authorization", format!("Bearer {}", key));
    }
    let req = builder.body(Full::new(Bytes::from(payload.to_string()))).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let res: hyper::Response<Incoming> = client.request(req).await.map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let status = res.status();
    let body_bytes = res.into_body().collect().await.map_err(|e| ErrorData::internal_error(e.to_string(), None))?.to_bytes();
    let raw: serde_json::Value = serde_json::from_slice(&body_bytes).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    if !status.is_success() {
        return Err(ErrorData::internal_error(format!("status {} error: {}", status, raw), None));
    }
    let text = raw["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string();
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

// registration is embedded in tool()
