use crate::tools::ToolEntry;
use crate::tools::to_object;
use rmcp::{
    ErrorData,
    model::*,
    service::{RequestContext, RoleServer},
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

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let args: SumRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        // Request parameters via elicitation when not provided
        match context
            .peer
            .elicit::<SumRequest>("请提供两个数字用于计算".to_string())
            .await
        {
            Ok(Some(params)) => params,
            Ok(None) => return Err(ErrorData::invalid_params("未提供参数", None)),
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
    };

    let result = args.a + args.b;

    Ok(CallToolResult::success(vec![Content::text(
        result.to_string(),
    )]))
}

// registration is embedded in tool()
