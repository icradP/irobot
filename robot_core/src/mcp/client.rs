use async_trait::async_trait;
use serde_json::Value;
use crate::mcp::registry::ToolMeta;

#[async_trait]
pub trait MCPClient: Send + Sync {
    async fn call(&self, tool: &str, args: Value) -> anyhow::Result<Value>;
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolMeta>>;
    async fn required_fields(&self, tool: &str) -> anyhow::Result<Vec<String>>;
    async fn tool_schema(&self, tool: &str) -> anyhow::Result<Option<Value>>;
    async fn elicit_preview(&self, _tool: &str) -> anyhow::Result<Option<Value>> {
        Ok(None)
    }
}

pub struct BasicMCPClient;

#[async_trait]
impl MCPClient for BasicMCPClient {
    async fn call(&self, _tool: &str, _args: Value) -> anyhow::Result<Value> {
        Ok(Value::Null)
    }
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolMeta>> {
        Ok(Vec::new())
    }
    async fn required_fields(&self, _tool: &str) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }
    async fn tool_schema(&self, _tool: &str) -> anyhow::Result<Option<Value>> {
        Ok(None)
    }
    async fn elicit_preview(&self, _tool: &str) -> anyhow::Result<Option<Value>> {
        Ok(None)
    }
}
