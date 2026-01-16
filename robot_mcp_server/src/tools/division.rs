use crate::tools::to_object;
use crate::tools::ToolEntry;
use rmcp::{
    ErrorData,
    model::*,
    service::{ElicitationError, RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Division of two numbers")]
pub struct DivisionRequest {
    #[schemars(description = "Dividend")]
    pub a: f64,
    #[schemars(description = "Divisor")]
    pub b: f64,
}

impl rmcp::service::ElicitationSafe for DivisionRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Division 引导参数")]
pub struct DivisionElicitation {
    #[schemars(description = "Dividend")]
    pub a: Option<f64>,
    #[schemars(description = "Divisor")]
    pub b: Option<f64>,
}

impl rmcp::service::ElicitationSafe for DivisionElicitation {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(DivisionRequest);
    let tool = Tool {
        name: "division".into(),
        title: Some("Division".into()),
        description: Some("[Utility] Calculate the division of two numbers".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "division",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

fn normalize_f64(v: Option<f64>) -> Option<f64> {
    v
}

fn parse_f64_value(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let mut a: Option<f64> = None;
    let mut b: Option<f64> = None;
    let mut max_attempts: usize = 5;
    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = m.clamp(1, 20) as usize;
        }
        if let Ok(parsed) = serde_json::from_value::<DivisionRequest>(args.clone()) {
            a = Some(parsed.a);
            b = Some(parsed.b);
        } else {
            a = args.get("a").and_then(parse_f64_value);
            b = args.get("b").and_then(parse_f64_value);
        }
    }

    let mut prompt = "请提供两个数字用于除法计算，且除数不能为0".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=division\nmessage=用户取消了本次工具调用",
            )]));
        }
        a = normalize_f64(a);
        b = normalize_f64(b);
        if let (Some(a_val), Some(b_val)) = (a, b) {
            if b_val == 0.0 {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_error\nname=division\nmessage=除数不能为0".to_string(),
                )]));
            } else {
                let result = a_val / b_val;
                return Ok(CallToolResult::success(vec![Content::text(
                    result.to_string(),
                )]));
            }
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=division\nmessage=用户取消了本次工具调用",
                )]));
            }
            r = context.peer.elicit::<DivisionElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                a = params.a;
                b = params.b;
                prompt = "请提供两个数字用于除法计算，且除数不能为0".to_string();
            }
            Ok(None) => {
                prompt = "未获得输入，请重新提供两个数字用于除法计算".to_string();
                a = None;
                b = None;
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=division\nmessage=用户取消了本次工具调用",
                )]));
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=division\nmessage=用户拒绝提供参数，本次工具调用已取消",
                )]));
            }
            Err(ElicitationError::ParseError { .. }) => {
                prompt = "输入格式不符合要求，请重新提供两个数字用于除法计算".to_string();
                a = None;
                b = None;
            }
            Err(ElicitationError::NoContent) => {
                prompt = "未收到有效内容，请重新提供两个数字用于除法计算".to_string();
                a = None;
                b = None;
            }
            Err(e) => {
                return Err(ErrorData::internal_error(
                    format!("引导错误: {}", e),
                    None,
                ))
            }
        }
    }

    Ok(CallToolResult::success(vec![Content::text(format!(
        "tool_error\nname=division\nmessage=缺参引导已达到上限({})，仍未获得两个有效数字用于除法计算",
        max_attempts
    ))]))
}
