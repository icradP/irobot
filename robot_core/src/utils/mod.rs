use crate::core::persona::{OutputStyle, Persona};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InputEvent {
    pub id: Uuid,
    pub source: String,
    pub session_id: Option<String>,
    pub source_meta: Option<crate::core::input_handler::SourceMetadata>,
    pub payload: Value,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputEvent {
    pub target: String,
    pub source: String, // Track which input source this output is for
    pub session_id: Option<String>,
    pub content: Value,
    pub style: OutputStyle,
}

impl OutputEvent {
    pub fn from_context(ctx: &Context) -> Self {
        Self {
            target: "default".to_string(),
            source: "system".to_string(), // Default source
            session_id: ctx.session_id.clone(),
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
    pub session_id: Option<String>,
}

impl Context {
    pub fn new(persona: Persona, input_text: String, session_id: Option<String>) -> Self {
        Self {
            persona,
            memory: Value::Null,
            profile: Value::Null,
            relationships: Value::Null,
            input_text,
            session_id,
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowPlan {
    pub steps: Vec<StepSpec>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum StepSpec {
    Memory,
    Profile,
    Relationship,
    Tool {
        name: String,
        args: Value,
        #[serde(default)]
        is_background: bool,
    },
}

static EVENT_BUS_SENDER: OnceLock<broadcast::Sender<InputEvent>> = OnceLock::new();
static OUTPUT_BUS_SENDER: OnceLock<broadcast::Sender<OutputEvent>> = OnceLock::new();
static CONSUMED_EVENTS: OnceLock<Mutex<HashSet<Uuid>>> = OnceLock::new();
static ACTIVE_ELICITATIONS: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();

pub fn event_bus() -> broadcast::Sender<InputEvent> {
    EVENT_BUS_SENDER
        .get_or_init(|| {
            let (tx, _rx) = broadcast::channel(1024);
            tx
        })
        .clone()
}

pub fn output_bus() -> broadcast::Sender<OutputEvent> {
    OUTPUT_BUS_SENDER
        .get_or_init(|| {
            let (tx, _rx) = broadcast::channel(1024);
            tx
        })
        .clone()
}

pub fn mark_event_consumed(id: Uuid) {
    let set = CONSUMED_EVENTS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut guard) = set.lock() {
        guard.insert(id);
    }
}

pub fn check_and_remove_consumed_event(id: &Uuid) -> bool {
    let set = CONSUMED_EVENTS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut guard) = set.lock() {
        return guard.remove(id);
    }
    false
}

pub fn set_elicitation_active(session_id: &str, active: bool) {
    let set = ACTIVE_ELICITATIONS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(mut guard) = set.lock() {
        if active {
            guard.insert(session_id.to_string());
        } else {
            guard.remove(session_id);
        }
    }
}

pub fn is_elicitation_active(session_id: &str) -> bool {
    let set = ACTIVE_ELICITATIONS.get_or_init(|| Mutex::new(HashSet::new()));
    if let Ok(guard) = set.lock() {
        guard.contains(session_id)
    } else {
        false
    }
}
