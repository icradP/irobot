use std::sync::Arc;
use async_trait::async_trait;
use serde_json::Value;
use crate::mcp::client::MCPClient;
use crate::mcp::registry::ToolMeta;
use super::manager::TaskManager;

pub struct TaskAwareMcpClient {
    inner: Arc<dyn MCPClient + Send + Sync>,
    task_manager: Arc<TaskManager>,
}

impl TaskAwareMcpClient {
    pub fn new(inner: Arc<dyn MCPClient + Send + Sync>, task_manager: Arc<TaskManager>) -> Self {
        Self { inner, task_manager }
    }

    async fn call_tool_inner(&self, name: &str, args: Value) -> anyhow::Result<Value> {
        match name {
            "list_running_tasks" => {
                let tasks = self.task_manager.list_tasks().await;
                Ok(serde_json::to_value(rmcp::model::CallToolResult::success(vec![
                    rmcp::model::Content::text(serde_json::to_string_pretty(&tasks)?),
                ]))?)
            }
            "cancel_task" => {
                let task_id = args.get("task_id")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing required argument: task_id"))?;
                
                let success = self.task_manager.cancel_task(task_id).await;
                let msg = if success {
                    format!("Task {} cancelled successfully", task_id)
                } else {
                    format!("Task {} not found", task_id)
                };
                
                Ok(serde_json::to_value(rmcp::model::CallToolResult::success(vec![
                    rmcp::model::Content::text(msg),
                ]))?)
            }
            _ => self.inner.call(name, args).await,
        }
    }
}

#[async_trait]
impl MCPClient for TaskAwareMcpClient {
    async fn list_tools(&self) -> anyhow::Result<Vec<ToolMeta>> {
        let mut tools = self.inner.list_tools().await?;
        
        tools.push(ToolMeta {
            name: "list_running_tasks".to_string(),
            description: "CRITICAL: Call this tool FIRST when the user wants to check status or cancel a task. Returns a list of tasks with 'ordinal' (index), 'original_prompt' (user intent), and 'task_id'. Use this output to map user's natural language description to a precise 'task_id'.".to_string(),
            is_long_running: false,
        });
        
        tools.push(ToolMeta {
            name: "cancel_task".to_string(),
            description: "Cancels a background task. REQUIRED: You MUST have a valid 'task_id' from the output of 'list_running_tasks' before calling this. DO NOT guess the ID. If you don't know the ID, call 'list_running_tasks' first.".to_string(),
            is_long_running: false,
        });

        Ok(tools)
    }

    async fn call(&self, tool: &str, args: Value) -> anyhow::Result<Value> {
        self.call_tool_inner(tool, args).await
    }

    async fn required_fields(&self, tool: &str) -> anyhow::Result<Vec<String>> {
        match tool {
            "cancel_task" => Ok(vec!["task_id".to_string()]),
            "list_running_tasks" => Ok(vec![]),
            _ => self.inner.required_fields(tool).await,
        }
    }

    async fn tool_schema(&self, tool: &str) -> anyhow::Result<Option<Value>> {
        match tool {
            "cancel_task" => Ok(Some(serde_json::json!({
                "type": "object",
                "properties": {
                    "task_id": {
                        "type": "string",
                        "description": "The ID of the task to cancel. Must be exactly as returned by list_running_tasks."
                    }
                },
                "required": ["task_id"]
            }))),
            "list_running_tasks" => Ok(Some(serde_json::json!({
                "type": "object",
                "properties": {},
            }))),
            _ => self.inner.tool_schema(tool).await,
        }
    }
}
