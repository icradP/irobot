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
#[schemars(description = "Sum of two numbers")]
pub struct SumRequest {
    #[schemars(description = "First number")]
    pub a: i32,
    #[schemars(description = "Second number")]
    pub b: i32,
}

impl rmcp::service::ElicitationSafe for SumRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Sum 引导参数")]
pub struct SumElicitation {
    #[schemars(description = "First number")]
    pub a: Option<i32>,
    #[schemars(description = "Second number")]
    pub b: Option<i32>,
}

impl rmcp::service::ElicitationSafe for SumElicitation {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(SumRequest);
    let tool = Tool {
        name: "sum".into(),
        title: Some("Sum".into()),
        description: Some("[Utility] Calculate the sum of two numbers".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "sum",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

fn normalize_i32(v: Option<i32>) -> Option<i32> {
    v
}

fn parse_i32_value(v: &serde_json::Value) -> Option<i32> {
    match v {
        serde_json::Value::Number(n) => n.as_i64().and_then(|i| i32::try_from(i).ok()),
        serde_json::Value::String(s) => s.trim().parse::<i32>().ok(),
        _ => None,
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let mut a: Option<i32> = None;
    let mut b: Option<i32> = None;
    let mut max_attempts: usize = 5;
    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = (m.clamp(1, 20)) as usize;
        }
        if let Ok(parsed) = serde_json::from_value::<SumRequest>(args.clone()) {
            a = Some(parsed.a);
            b = Some(parsed.b);
        } else {
            a = args.get("a").and_then(parse_i32_value);
            b = args.get("b").and_then(parse_i32_value);
        }
    }

    let mut prompt = "请提供两个数字用于计算".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=sum\nmessage=用户取消了本次工具调用",
            )]));
        }
        a = normalize_i32(a);
        b = normalize_i32(b);
        if let (Some(a), Some(b)) = (a, b) {
            let result = a + b;
            return Ok(CallToolResult::success(vec![Content::text(result.to_string())]));
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=sum\nmessage=用户取消了本次工具调用",
                )]));
            }
            r = context.peer.elicit::<SumElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                a = params.a;
                b = params.b;
                prompt = "请提供两个数字用于计算".to_string();
            }
            Ok(None) => {
                prompt = "未获得输入，请重新提供两个数字".to_string();
                a = None;
                b = None;
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=sum\nmessage=用户取消了本次工具调用",
                )]))
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=sum\nmessage=用户拒绝提供参数，本次工具调用已取消",
                )]))
            }
            Err(ElicitationError::ParseError { .. }) => {
                prompt = "输入格式不符合要求，请重新提供两个数字".to_string();
                a = None;
                b = None;
            }
            Err(ElicitationError::NoContent) => {
                prompt = "未收到有效内容，请重新提供两个数字".to_string();
                a = None;
                b = None;
            }
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
    }

    Ok(CallToolResult::success(vec![Content::text(format!(
        "tool_error\nname=sum\nmessage=缺参引导已达到上限({})，仍未获得两个有效数字",
        max_attempts
    ))]))
}

// registration is embedded in tool()
