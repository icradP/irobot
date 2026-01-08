use crate::tools::to_object;
use rmcp::{
    model::*,
    service::{RequestContext, RoleServer},
    ErrorData,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use crate::tools::ToolEntry;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Echo message request")]
pub struct EchoRequest {
    #[schemars(description = "Message to echo back")]
    pub message: String,
}

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
    let args: EchoRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        // Request message via elicitation when not provided
        match context
            .peer
            .elicit::<EchoRequest>("请提供要回显的消息".to_string())
            .await
        {
            Ok(Some(params)) => params,
            Ok(None) => return Err(ErrorData::invalid_params("未提供消息", None)),
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
    };

    Ok(CallToolResult::success(vec![Content::text(args.message)]))
}

// registration is embedded in tool()
