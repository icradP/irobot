use crate::tools::ToolEntry;
use crate::tools::to_object;
use rmcp::{
    ErrorData,
    model::*,
    service::{ElicitationError, RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Echo message request")]
pub struct EchoRequest {
    #[schemars(description = "Message to echo back")]
    pub message: String,
}

impl rmcp::service::ElicitationSafe for EchoRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Echo message elicitation")]
pub struct EchoElicitation {
    #[schemars(description = "Message to echo back")]
    pub message: Option<String>,
}
impl rmcp::service::ElicitationSafe for EchoElicitation {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(EchoRequest);
    let tool = Tool {
        name: "echo".into(),
        title: Some("Echo".into()),
        description: Some("[Utility] Returns the exact input string provided. Useful for testing connectivity or verification.".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None, annotations: None, icons: None, meta: None,
    };
    ToolEntry {
        name: "echo",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let mut message: Option<String> = None;
    let mut max_attempts: usize = 5;

    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = (m.clamp(1, 20)) as usize;
        }

        if let Ok(parsed) = serde_json::from_value::<EchoRequest>(args.clone()) {
            message = Some(parsed.message);
        } else {
            if let Some(m) = args.get("message").and_then(|v| v.as_str()) {
                message = Some(m.to_string());
            }
        }
    }

    let mut prompt = "请提供要回显的消息".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=echo\nmessage=用户取消了回显请求",
            )]));
        }

        if message.is_some() {
            break;
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=echo\nmessage=用户取消了回显请求",
                )]));
            }
            r = context.peer.elicit::<EchoElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                if let Some(m) = params.message {
                    if !m.trim().is_empty() {
                         message = Some(m);
                    }
                }
                if message.is_none() {
                    prompt = "仍缺少必要参数(message)，请补充：".to_string();
                }
            }
             Ok(None) => {
                prompt = "未获得有效消息，请重新提供".to_string();
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=echo\nmessage=用户取消了回显请求",
                )]))
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=echo\nmessage=用户拒绝提供参数，请求已取消",
                )]))
            }
            Err(ElicitationError::ParseError { .. }) => {
                prompt = "输入格式不符合要求，请重新提供".to_string();
            }
            Err(ElicitationError::NoContent) => {
                prompt = "未收到有效内容，请重新提供".to_string();
            }
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
        
        if message.is_some() {
            break;
        }
    }
    
    if message.is_none() {
         return Ok(CallToolResult::success(vec![Content::text(format!("tool_error\nname=echo\nmessage=缺参引导已达到上限({})，仍未获得有效的消息", max_attempts))]));
    }
    
    Ok(CallToolResult::success(vec![Content::text(message.unwrap())]))
}

// registration is embedded in tool()
