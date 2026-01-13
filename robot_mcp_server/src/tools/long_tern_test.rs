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
use tokio::time::{sleep, Duration};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Long running task test")]
pub struct LongTermTestRequest {
    #[schemars(description = "Number of steps")]
    pub count: u32,
    #[schemars(description = "Delay per step in ms")]
    pub delay_ms: Option<u64>,
    #[schemars(description = "Progress token to use")]
    pub progress_token: Option<String>,
}

impl rmcp::service::ElicitationSafe for LongTermTestRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Long Term Test 引导参数")]
pub struct LongTermTestElicitation {
    #[schemars(description = "Number of steps")]
    pub count: Option<u32>,
    #[schemars(description = "Delay per step in ms")]
    pub delay_ms: Option<u64>,
    #[schemars(description = "Progress token to use")]
    pub progress_token: Option<String>,
}

impl rmcp::service::ElicitationSafe for LongTermTestElicitation {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(LongTermTestRequest);
    let tool = Tool {
        name: "long_term_test".into(),
        title: Some("Long Term Test".into()),
        description: Some("Simulate a long running task with progress updates".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "long_term_test",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let mut count: Option<u32> = None;
    let mut delay_ms: Option<u64> = None;
    let mut progress_token: Option<String> = None;
    let mut max_attempts: usize = 5;

    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = (m.clamp(1, 20)) as usize;
        }
        if let Ok(parsed) = serde_json::from_value::<LongTermTestRequest>(args.clone()) {
            count = Some(parsed.count);
            delay_ms = parsed.delay_ms;
            progress_token = parsed.progress_token;
        } else {
            // Manual parsing if full struct parsing fails
            if let Some(c) = args.get("count").and_then(|v| v.as_u64()) {
                count = Some(c as u32);
            }
            if let Some(d) = args.get("delay_ms").and_then(|v| v.as_u64()) {
                delay_ms = Some(d);
            }
            if let Some(t) = args.get("progress_token").and_then(|v| v.as_str()) {
                progress_token = Some(t.to_string());
            }
        }
    }

    let mut prompt = "请提供长任务的执行步数".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=long_term_test\nmessage=用户取消了本次工具调用",
            )]));
        }

        if let Some(c) = count {
            let delay = delay_ms.unwrap_or(1000);
            let token = progress_token.clone().unwrap_or_else(|| "long_term_test_progress".to_string());
            let token_val = match token.parse::<i64>() {
                Ok(n) => NumberOrString::Number(n),
                Err(_) => NumberOrString::String(token.into()),
            };

            for i in 1..=c {
                if context.ct.is_cancelled() {
                    return Ok(CallToolResult::success(vec![Content::text("Cancelled")]));
                }
                sleep(Duration::from_millis(delay)).await;

                let _ = context.peer.notify_progress(ProgressNotificationParam {
                    progress_token: ProgressToken(token_val.clone()),
                    progress: i as f64,
                    total: Some(c as f64),
                    message: Some(format!("Step {}/{}", i, c)),
                }).await;
            }

            return Ok(CallToolResult::success(vec![Content::text(format!("Completed {} steps", c))]));
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=long_term_test\nmessage=用户取消了本次工具调用",
                )]));
            }
            r = context.peer.elicit::<LongTermTestElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                if let Some(c) = params.count {
                    count = Some(c);
                }
                if let Some(d) = params.delay_ms {
                    delay_ms = Some(d);
                }
                if let Some(t) = params.progress_token {
                    progress_token = Some(t);
                }
                prompt = "请提供长任务的执行步数".to_string();
            }
            Ok(None) => {
                prompt = "未获得输入，请重新提供执行步数".to_string();
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=long_term_test\nmessage=用户取消了本次工具调用",
                )]))
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=long_term_test\nmessage=用户拒绝提供参数，本次工具调用已取消",
                )]))
            }
            Err(ElicitationError::ParseError { .. }) => {
                prompt = "输入格式不符合要求，请重新提供执行步数".to_string();
            }
            Err(ElicitationError::NoContent) => {
                prompt = "未收到有效内容，请重新提供执行步数".to_string();
            }
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
    }

    Ok(CallToolResult::success(vec![Content::text(format!(
        "tool_error\nname=long_term_test\nmessage=缺参引导已达到上限({})，仍未获得有效参数",
        max_attempts
    ))]))
}
