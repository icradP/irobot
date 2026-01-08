use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolMeta {
    pub name: String,
    pub description: String,
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<ToolMeta>,
}

impl ToolRegistry {
    pub fn register(&mut self, meta: ToolMeta) {
        self.tools.push(meta);
    }
    pub fn list(&self) -> &[ToolMeta] {
        &self.tools
    }
}

