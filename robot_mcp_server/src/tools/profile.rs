use schemars::JsonSchema;
use serde::{Serialize, Deserialize};
use rmcp::{ErrorData, model::*, service::{RoleServer, RequestContext}};
use std::sync::{Arc, Mutex};
use crate::tools::{to_object, AppState, ToolEntry};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProfileUpdateRequest { pub profile: serde_json::Value }
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProfileGetRequest {}

pub fn update_tool() -> ToolEntry {
    let schema = schemars::schema_for!(ProfileUpdateRequest);
    let tool = Tool {
        name: "profile_update".into(),
        title: Some("Update Profile".into()),
        description: Some("Update the user profile with a JSON object".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None, annotations: None, icons: None, meta: None,
    };
    ToolEntry {
        name: "profile_update",
        tool,
        handler: Arc::new(|request, context, state| Box::pin(async move { update_handle(request, context, &state).await })),
    }
}
pub fn get_tool() -> ToolEntry {
    let schema = schemars::schema_for!(ProfileGetRequest);
    let tool = Tool {
        name: "profile_get".into(),
        title: Some("Get Profile".into()),
        description: Some("[Profile] Get the current user profile".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None, annotations: None, icons: None, meta: None,
    };
    ToolEntry {
        name: "profile_get",
        tool,
        handler: Arc::new(|request, context, state| Box::pin(async move { get_handle(request, context, &state).await })),
    }
}

pub async fn update_handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
    state: &Arc<Mutex<AppState>>,
) -> Result<CallToolResult, ErrorData> {
    let args: ProfileUpdateRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        match context.peer.elicit::<ProfileUpdateRequest>("请输入画像 JSON".to_string()).await {
            Ok(Some(params)) => params,
            Ok(None) => return Err(ErrorData::invalid_params("未提供参数", None)),
            Err(e) => return Err(ErrorData::internal_error(format!("elicitation 错误: {}", e), None)),
        }
    };
    let mut s = state.lock().unwrap();
    s.profile = args.profile.clone();
    Ok(CallToolResult::success(vec![Content::text("Profile updated")]))
}

pub async fn get_handle(
    _request: Option<serde_json::Value>,
    _context: RequestContext<RoleServer>,
    state: &Arc<Mutex<AppState>>,
) -> Result<CallToolResult, ErrorData> {
    let s = state.lock().unwrap();
    let json = serde_json::to_string_pretty(&s.profile).unwrap_or_default();
    Ok(CallToolResult::success(vec![Content::text(json)]))
}

// registration is embedded in update_tool/get_tool
