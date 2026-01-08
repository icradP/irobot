use schemars::JsonSchema;
use serde::{Serialize, Deserialize};
use rmcp::{ErrorData, model::*, service::{RoleServer, RequestContext}};
use std::sync::{Arc, Mutex};
use crate::tools::{to_object, AppState, ToolEntry};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MemorySaveRequest { pub key: String, pub value: String }
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MemoryRecallRequest { pub key: String }

pub fn save_tool() -> ToolEntry {
    let schema = schemars::schema_for!(MemorySaveRequest);
    let tool = Tool {
        name: "memory_save".into(),
        title: Some("Save Memory".into()),
        description: Some("[Memory] Save a value to memory with a key".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None, annotations: None, icons: None, meta: None,
    };
    ToolEntry {
        name: "memory_save",
        tool,
        handler: Arc::new(|request, context, state| Box::pin(async move { save_handle(request, context, &state).await })),
    }
}
pub fn recall_tool() -> ToolEntry {
    let schema = schemars::schema_for!(MemoryRecallRequest);
    let tool = Tool {
        name: "memory_recall".into(),
        title: Some("Recall Memory".into()),
        description: Some("[Memory] Recall a value from memory by key".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None, annotations: None, icons: None, meta: None,
    };
    ToolEntry {
        name: "memory_recall",
        tool,
        handler: Arc::new(|request, context, state| Box::pin(async move { recall_handle(request, context, &state).await })),
    }
}

pub async fn save_handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
    state: &Arc<Mutex<AppState>>,
) -> Result<CallToolResult, ErrorData> {
    let args: MemorySaveRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        match context.peer.elicit::<MemorySaveRequest>("请输入要保存的 key 和 value".to_string()).await {
            Ok(Some(params)) => params,
            Ok(None) => return Err(ErrorData::invalid_params("未提供参数", None)),
            Err(e) => return Err(ErrorData::internal_error(format!("elicitation 错误: {}", e), None)),
        }
    };
    let mut s = state.lock().unwrap();
    s.memory.insert(args.key.clone(), args.value.clone());
    Ok(CallToolResult::success(vec![Content::text(format!("Saved memory: {} = {}", args.key, args.value))]))
}

pub async fn recall_handle(
    request: Option<serde_json::Value>,
    _context: RequestContext<RoleServer>,
    state: &Arc<Mutex<AppState>>,
) -> Result<CallToolResult, ErrorData> {
    let args: MemoryRecallRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        return Err(ErrorData::invalid_params("未提供参数", None));
    };
    let s = state.lock().unwrap();
    let value = s.memory.get(&args.key).cloned().unwrap_or_else(|| "Not found".to_string());
    Ok(CallToolResult::success(vec![Content::text(value)]))
}

// registration is embedded in save_tool/recall_tool
