use crate::core::persona::{OutputStyle, Persona};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputEvent {
    pub id: Uuid,
    pub source: String,
    pub source_meta: Option<crate::core::input_handler::SourceMetadata>,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputEvent {
    pub target: String,
    pub source: String, // Track which input source this output is for
    pub content: Value,
    pub style: OutputStyle,
}

impl OutputEvent {
    pub fn from_context(ctx: &Context) -> Self {
        Self {
            target: "default".to_string(),
            source: "system".to_string(), // Default source
            content: serde_json::json!({
                "persona": ctx.persona.name,
                "memory": ctx.memory,
                "profile": ctx.profile,
                "relationships": ctx.relationships
            }),
            style: ctx.persona.style.clone(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Context {
    pub persona: Persona,
    pub memory: Value,
    pub profile: Value,
    pub relationships: Value,
    pub input_text: String,
}

impl Context {
    pub fn new(persona: Persona, input_text: String) -> Self {
        Self {
            persona,
            memory: Value::Null,
            profile: Value::Null,
            relationships: Value::Null,
            input_text,
        }
    }
    pub fn touch_memory(&mut self) {
        self.memory = serde_json::json!({"touched": true});
    }
    pub fn touch_profile(&mut self) {
        self.profile = serde_json::json!({"touched": true});
    }
    pub fn touch_relationships(&mut self) {
        self.relationships = serde_json::json!({"touched": true});
    }
}

#[derive(Clone, Debug)]
pub struct WorkflowPlan {
    pub steps: Vec<StepSpec>,
}

#[derive(Clone, Debug)]
pub enum StepSpec {
    Memory,
    Profile,
    Relationship,
    Tool { name: String, args: Value },
}
