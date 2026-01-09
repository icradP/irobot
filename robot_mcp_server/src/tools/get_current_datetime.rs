//获取当前日期时间
use crate::tools::ToolEntry;
use crate::tools::to_object;
use chrono::Local;
use rmcp::{
    ErrorData,
    model::*,
    service::{RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Get current datetime")]
pub struct GetCurrentDatetimeRequest;
impl rmcp::service::ElicitationSafe for GetCurrentDatetimeRequest {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(GetCurrentDatetimeRequest);
    let tool = Tool {
        name: "get_current_datetime".into(),
        title: Some("Get Current Datetime".into()),
        description: Some("[Utility] Get the current datetime".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "get_current_datetime",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

pub async fn handle(
    _request: Option<serde_json::Value>,
    _context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    Ok(CallToolResult::success(vec![Content::text(now)]))
}

// registration is embedded in tool()
