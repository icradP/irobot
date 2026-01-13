use rmcp::{
    ErrorData,
    model::*,
    service::{RequestContext, RoleServer},
};
use std::sync::{Arc, Mutex};
use std::{collections::HashMap, future::Future, pin::Pin};

pub fn to_object(v: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m,
        _ => serde_json::Map::new(),
    }
}

#[derive(Debug, Clone, Default)]
pub struct AppState {
    pub memory: HashMap<String, String>,
    pub profile: serde_json::Value,
}

pub mod chat;
pub mod echo;
pub mod ffprobe;
pub mod get_current_datetime;
pub mod get_weather;
pub mod long_tern_test;
pub mod profile;
pub mod sum;

pub type HandlerFuture = Pin<Box<dyn Future<Output = Result<CallToolResult, ErrorData>> + Send>>;

pub struct ToolEntry {
    pub name: &'static str,
    pub tool: Tool,
    pub handler: Arc<
        dyn Fn(
                Option<serde_json::Value>,
                RequestContext<RoleServer>,
                Arc<Mutex<AppState>>,
            ) -> HandlerFuture
            + Send
            + Sync,
    >,
}

pub fn all_entries() -> Vec<ToolEntry> {
    vec![
        echo::tool(),
        sum::tool(),
        profile::update_tool(),
        profile::get_tool(),
        chat::tool(),
        get_weather::tool(),
        get_current_datetime::tool(),
        ffprobe::tool(),
        long_tern_test::tool(),
    ]
}

pub fn all_tools() -> Vec<Tool> {
    all_entries().into_iter().map(|e| e.tool).collect()
}

pub async fn dispatch(
    name: &str,
    args: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
    state: Arc<Mutex<AppState>>,
) -> Result<CallToolResult, ErrorData> {
    for entry in all_entries() {
        if entry.name == name {
            return (entry.handler)(args, context, state).await;
        }
    }
    Err(ErrorData::invalid_params(
        format!("未知工具: {}", name),
        None,
    ))
}
