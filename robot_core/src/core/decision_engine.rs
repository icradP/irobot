use crate::core::persona::Persona;
use crate::llm::adapter::{ChatMessage, ChatRequest, LLMClient};
use crate::mcp::client::MCPClient;
use crate::mcp::registry::ToolMeta;
use crate::utils::{InputEvent, StepSpec, WorkflowPlan};
use async_trait::async_trait;
use serde_json::Value;
use tracing::info;

#[async_trait]
pub trait DecisionEngine {
    async fn decide(&self, persona: &Persona, input: &InputEvent) -> anyhow::Result<WorkflowPlan>;
}

pub struct BasicDecisionEngine;

#[async_trait]
impl DecisionEngine for BasicDecisionEngine {
    async fn decide(
        &self,
        _persona: &Persona,
        _input: &InputEvent,
    ) -> anyhow::Result<WorkflowPlan> {
        let plan = WorkflowPlan {
            steps: vec![StepSpec::Memory, StepSpec::Profile, StepSpec::Relationship],
        };
        info!("decision_engine plan: {:?}", plan);
        Ok(plan)
    }
}

use std::sync::Arc;

pub struct LLMDecisionEngine {
    pub llm: Box<dyn LLMClient + Send + Sync>,
    pub model: String,
    pub mcp: Arc<dyn MCPClient + Send + Sync>,
}

impl LLMDecisionEngine {
    pub fn new(
        llm: Box<dyn LLMClient + Send + Sync>,
        model: String,
        mcp: Arc<dyn MCPClient + Send + Sync>,
    ) -> Self {
        Self { llm, model, mcp }
    }
}

#[async_trait]
impl DecisionEngine for LLMDecisionEngine {
    async fn decide(&self, _persona: &Persona, input: &InputEvent) -> anyhow::Result<WorkflowPlan> {
        let tools: Vec<ToolMeta> = self.mcp.list_tools().await.unwrap_or_default();
        // 结构化打印工具列表
        let tool_list: Vec<String> = tools.iter().enumerate()
            .map(|(idx, tool)| format!("[{}] name={} description={}", idx, tool.name, tool.description))
            .collect();
        info!("DecisionEngine detected MCP tools: [{}]", tool_list.join(", "));
        let tool_descriptions: Vec<String> = tools
            .iter()
            .map(|t| format!("{}: {}", t.name, t.description))
            .collect();

        // Extract text based on source metadata or fallback to known patterns
        let text = if let Some(meta) = &input.source_meta {
            input
                .payload
                .get(&meta.content_field)
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        } else {
            // Fallback to console format
            input
                .payload
                .get("line")
                .or_else(|| input.payload.get("content"))
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string()
        };

        let source_context = if let Some(meta) = &input.source_meta {
            format!(
                "Input Source: {}\nFormat: {}\nDescription: {}\n",
                meta.name, meta.format_hint, meta.description
            )
        } else {
            "Input Source: unknown\n".to_string()
        };

        let system = format!(
            "{}You are a smart workflow planner. Your goal is to select the minimal and optimal set of tools to fulfill the user's request.\n\
            Available Steps: [\"Memory\",\"Profile\",\"Relationship\"].\n\
            Available MCP Tools: {:?}.\n\
            \n\
            Tool Categories:\n\
            - [Conversational]: For natural language interaction, chat, Q&A, and roleplay.\n\
            - [Utility]: For specific calculations, data processing, testing, or system operations.\n\
            - [Memory]: For storing and recalling long-term information.\n\
            - [Profile]: For managing user profiles and preferences.\n\
            \n\
            Rules:\n\
            1. Analyze the user's intent and match it to the appropriate Tool Category.\n\
            2. If the user's input is casual conversation (greeting, small talk, general questions), prioritize [Conversational] tools.\n\
            3. Use [Utility] tools ONLY when the user explicitly requests that specific functionality (e.g., math, echo).\n\
            4. Use [Memory] or [Profile] tools if the request involves remembering facts or accessing user data.\n\
            5. Choose ONLY the necessary tools. Avoid redundant steps.\n\
            6. Return a pure JSON array of strings representing the sequence of steps. No explanation.",
            source_context, tool_descriptions
        );
        let user = format!("Input: {}\nReturn steps:", text);
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: system.into(),
                },
                ChatMessage {
                    role: "user".into(),
                    content: user,
                },
            ],
            temperature: Some(0.2),
        };
        let out = self.llm.chat(req).await?;
        let s = out.text.trim();
        let json_slice = match (s.find('['), s.rfind(']')) {
            (Some(i), Some(j)) if j >= i => &s[i..=j],
            _ => s,
        };
        let names: Vec<String> = serde_json::from_str(json_slice).unwrap_or_default();
        let mut steps = Vec::new();
        for n in names {
            let args: Value = Value::Null;
            let lower = n.to_lowercase();
            if lower == "memory" {
                steps.push(StepSpec::Memory);
            } else if lower == "profile" {
                steps.push(StepSpec::Profile);
            } else if lower == "relationship" {
                steps.push(StepSpec::Relationship);
            } else {
                steps.push(StepSpec::Tool { name: n, args });
            }
        }
        let plan = WorkflowPlan { steps };
        info!("llm decision plan: {:?}", plan);
        Ok(plan)
    }
}
