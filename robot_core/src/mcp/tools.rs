use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub args: Value,
}

