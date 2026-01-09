use crate::tools::{ToolEntry, to_object};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use rmcp::{
    ErrorData,
    model::*,
    service::{ElicitationError, RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use url::Url;
// remove unused duplicated imports

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChatRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f32>,
}
impl rmcp::service::ElicitationSafe for ChatRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ChatElicitation {
    pub model: Option<String>,
    pub messages: Option<Vec<ChatMessage>>,
    pub temperature: Option<f32>,
}
impl rmcp::service::ElicitationSafe for ChatElicitation {}

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
    let mut model: Option<String> = None;
    let mut messages: Option<Vec<ChatMessage>> = None;
    let mut temperature: Option<f32> = None;
    let mut max_attempts: usize = 5;

    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = (m.clamp(1, 20)) as usize;
        }
        
        // Try strict parsing first, then loose
        if let Ok(parsed) = serde_json::from_value::<ChatRequest>(args.clone()) {
            model = parsed.model;
            messages = Some(parsed.messages);
            temperature = parsed.temperature;
        } else {
             if let Some(m) = args.get("model").and_then(|v| v.as_str()) {
                model = Some(m.to_string());
            }
            if let Some(msgs) = args.get("messages") {
                 if let Ok(m) = serde_json::from_value(msgs.clone()) {
                     messages = Some(m);
                 }
            }
            if let Some(t) = args.get("temperature").and_then(|v| v.as_f64()) {
                temperature = Some(t as f32);
            }
        }
    }

    let mut prompt = "请输入聊天的参数".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=chat\nmessage=用户取消了聊天请求",
            )]));
        }

        if messages.is_some() {
            break;
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=chat\nmessage=用户取消了聊天请求",
                )]));
            }
            r = context.peer.elicit::<ChatElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                if params.model.is_some() { model = params.model; }
                if let Some(msgs) = params.messages {
                    if !msgs.is_empty() {
                         messages = Some(msgs);
                    }
                }
                if params.temperature.is_some() { temperature = params.temperature; }
                
                if messages.is_none() {
                    prompt = "仍缺少必要参数(messages)，请补充：".to_string();
                }
            }
             Ok(None) => {
                prompt = "未获得有效参数，请重新提供".to_string();
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=chat\nmessage=用户取消了聊天请求",
                )]))
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=chat\nmessage=用户拒绝提供参数，请求已取消",
                )]))
            }
            Err(ElicitationError::ParseError { .. }) => {
                prompt = "输入格式不符合要求，请重新提供参数".to_string();
            }
            Err(ElicitationError::NoContent) => {
                prompt = "未收到有效内容，请重新提供".to_string();
            }
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
        
        if messages.is_some() {
            break;
        }
    }
    
    if messages.is_none() {
         return Ok(CallToolResult::success(vec![Content::text(format!("tool_error\nname=chat\nmessage=缺参引导已达到上限({})，仍未获得有效的聊天消息", max_attempts))]));
    }
    
    let args = ChatRequest {
        model,
        messages: messages.unwrap(),
        temperature,
    };
    let base =
        std::env::var("LMSTUDIO_URL").unwrap_or_else(|_| "http://localhost:1234".to_string());
    let api_key = std::env::var("LMSTUDIO_API_KEY").ok();
    let model = args.model.unwrap_or_else(|| {
        std::env::var("LMSTUDIO_MODEL").unwrap_or_else(|_| "default".to_string())
    });
    let mut endpoint =
        Url::parse(&base).map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
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
    let req = builder
        .body(Full::new(Bytes::from(payload.to_string())))
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let res: hyper::Response<Incoming> = client
        .request(req)
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    let status = res.status();
    let body_bytes = res
        .into_body()
        .collect()
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
        .to_bytes();
    let raw: serde_json::Value = serde_json::from_slice(&body_bytes)
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    if !status.is_success() {
        return Err(ErrorData::internal_error(
            format!("status {} error: {}", status, raw),
            None,
        ));
    }
    let text = raw["choices"][0]["message"]["content"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

// registration is embedded in tool()
