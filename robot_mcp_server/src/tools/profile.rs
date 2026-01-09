use crate::tools::{AppState, ToolEntry, to_object};
use rmcp::{
    ErrorData,
    model::*,
    service::{ElicitationError, RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProfileUpdateRequest {
    pub profile: serde_json::Value,
}
impl rmcp::service::ElicitationSafe for ProfileUpdateRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProfileUpdateElicitation {
    pub profile: Option<serde_json::Value>,
}
impl rmcp::service::ElicitationSafe for ProfileUpdateElicitation {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ProfileGetRequest {}
impl rmcp::service::ElicitationSafe for ProfileGetRequest {}

pub fn update_tool() -> ToolEntry {
    let schema = schemars::schema_for!(ProfileUpdateRequest);
    let tool = Tool {
        name: "profile_update".into(),
        title: Some("Update Profile".into()),
        description: Some("Update the user profile with a JSON object".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "profile_update",
        tool,
        handler: Arc::new(|request, context, state| {
            Box::pin(async move { update_handle(request, context, &state).await })
        }),
    }
}
pub fn get_tool() -> ToolEntry {
    let schema = schemars::schema_for!(ProfileGetRequest);
    let tool = Tool {
        name: "profile_get".into(),
        title: Some("Get Profile".into()),
        description: Some("[Profile] Get the current user profile".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "profile_get",
        tool,
        handler: Arc::new(|request, context, state| {
            Box::pin(async move { get_handle(request, context, &state).await })
        }),
    }
}

pub async fn update_handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
    state: &Arc<Mutex<AppState>>,
) -> Result<CallToolResult, ErrorData> {
    let mut profile: Option<serde_json::Value> = None;
    let mut max_attempts: usize = 5;

    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = (m.clamp(1, 20)) as usize;
        }

        if let Ok(parsed) = serde_json::from_value::<ProfileUpdateRequest>(args.clone()) {
            profile = Some(parsed.profile);
        } else {
            if let Some(m) = args.get("profile") {
                profile = Some(m.clone());
            }
        }
    }

    let mut prompt = "请输入画像 JSON".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=profile_update\nmessage=用户取消了画像更新请求",
            )]));
        }

        if profile.is_some() {
            break;
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=profile_update\nmessage=用户取消了画像更新请求",
                )]));
            }
            r = context.peer.elicit::<ProfileUpdateElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                if params.profile.is_some() {
                     profile = params.profile;
                }
                if profile.is_none() {
                    prompt = "仍缺少必要参数(profile)，请补充：".to_string();
                }
            }
             Ok(None) => {
                prompt = "未获得有效参数，请重新提供".to_string();
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=profile_update\nmessage=用户取消了画像更新请求",
                )]))
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=profile_update\nmessage=用户拒绝提供参数，请求已取消",
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
        
        if profile.is_some() {
            break;
        }
    }
    
    if profile.is_none() {
         return Ok(CallToolResult::success(vec![Content::text(format!("tool_error\nname=profile_update\nmessage=缺参引导已达到上限({})，仍未获得有效的画像 JSON", max_attempts))]));
    }
    
    let mut s = state.lock().unwrap();
    s.profile = profile.unwrap();
    Ok(CallToolResult::success(vec![Content::text(
        "Profile updated",
    )]))
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
