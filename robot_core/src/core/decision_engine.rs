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
    async fn decide(&self, persona: &Persona, input: &InputEvent, mcp_client: &dyn MCPClient) -> anyhow::Result<WorkflowPlan>;
}

pub struct BasicDecisionEngine;

#[async_trait]
impl DecisionEngine for BasicDecisionEngine {
    async fn decide(
        &self,
        _persona: &Persona,
        _input: &InputEvent,
        _mcp_client: &dyn MCPClient,
    ) -> anyhow::Result<WorkflowPlan> {
        let plan = WorkflowPlan {
            steps: vec![StepSpec::Memory, StepSpec::Profile, StepSpec::Relationship],
            reasoning: None,
        };
        info!("decision_engine plan: {:?}", plan);
        Ok(plan)
    }
}

use std::sync::Arc;

pub struct LLMDecisionEngine {
    pub llm: Box<dyn LLMClient + Send + Sync>,
    pub model: String,
}

impl LLMDecisionEngine {
    pub fn new(
        llm: Box<dyn LLMClient + Send + Sync>,
        model: String,
    ) -> Self {
        Self { llm, model }
    }
}

#[async_trait]
impl DecisionEngine for LLMDecisionEngine {
    async fn decide(&self, _persona: &Persona, input: &InputEvent, mcp_client: &dyn MCPClient) -> anyhow::Result<WorkflowPlan> {
        let tools: Vec<ToolMeta> = mcp_client.list_tools().await.unwrap_or_default();
        
        if tools.is_empty() {
            return Err(anyhow::anyhow!("NO_TOOLS_AVAILABLE"));
        }

        // 结构化打印工具列表
        let tool_list: Vec<String> = tools.iter().enumerate()
            .map(|(idx, tool)| format!("[{}] name={} description={}", idx, tool.name, tool.description))
            .collect();
        info!("DecisionEngine detected MCP tools:\n[{}]", tool_list.join("\n"));
        let tool_descriptions: Vec<String> = tools
            .iter()
            .map(|t| format!("name={} description={}", t.name, t.description))
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
            Available Steps: [\"Memory\"].\n\
            Available MCP Tools: {:?}.\n\
            \n\
            Tool Categories:
            - [Conversational]: For natural language interaction, chat, Q&A, and roleplay.
            - [Utility]: For specific calculations, data processing, testing, or system operations.
            - [Memory]: For storing and recalling long-term information.
            - [Profile]: For managing user profiles and preferences.
            - [System]: For background task management (listing, cancelling).
            
            Rules:
            1. Analyze the user's intent and match it to the appropriate Tool Category.
            2. If the user's input is casual conversation (greeting, small talk, general questions), prioritize [Conversational] tools.
            3. Use [Utility] tools ONLY when the user explicitly requests that specific functionality (e.g., math, echo).
            4. Use [Memory] or [Profile] tools if the request involves remembering facts or accessing user data.
            5. For task cancellation, you MUST include \"list_running_tasks\" BEFORE \"cancel_task\" in the sequence to identify the correct task ID.
            6. Choose ONLY the necessary tools. Avoid redundant steps.
            7. Perform multi-step reasoning. If a task requires the output of one tool to be used by another (e.g. \"calculate difference between A and B\"), include ALL necessary steps in logical order.
            8. IMPORTANT: If the user asks for a comparison or calculation based on retrieved data (e.g. \"time difference\"), you MUST include the calculation tool (e.g., \"sub\", \"sum\") after the retrieval tools.
            9. Return a JSON object with two fields: 'reasoning' (string) and 'steps' (array).
            - 'reasoning': Explain why you selected these tools and how you plan to extract parameters.
            - 'steps': The array of tool steps as described before. Each object must have 'tool' (string) and 'dependencies' (array of integers).
            'dependencies' should contain the 0-based indices of previous steps that the current step depends on. If independent, use [].
            Example:
            {{
              \"reasoning\": \"User wants to know time difference. I need to get current time twice (or user provided one?) and then subtract.\",
              \"steps\": [{{ \"tool\": \"get_current_datetime\", \"dependencies\": [] }}, {{ \"tool\": \"get_current_datetime\", \"dependencies\": [] }}, {{ \"tool\": \"sub\", \"dependencies\": [0, 1] }}]
            }}
            No explanation.",
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
            session_id: input.session_id.clone(),
        };
        let out = self.llm.chat(req).await?;

        let s = out.text.trim();
        let mut planner_reasoning = out.thought.clone();

        #[derive(serde::Deserialize)]
        struct StepItem {
            tool: String,
            dependencies: Vec<usize>,
        }

        #[derive(serde::Deserialize)]
        struct PlanResponse {
            reasoning: Option<String>,
            steps: Vec<StepItem>,
        }
        
        let mut step_items: Vec<StepItem> = Vec::new();

        // 1. Try to parse as JSON Object (New Format)
        let starts_obj: Vec<usize> = s.match_indices('{').map(|(i, _)| i).collect();
        for start in starts_obj {
            if let Some(end_offset) = s[start..].rfind('}') {
                let end = start + end_offset;
                let candidate = &s[start..=end];
                if let Ok(resp) = serde_json::from_str::<PlanResponse>(candidate) {
                    step_items = resp.steps;
                    if let Some(r) = resp.reasoning {
                        if !r.trim().is_empty() {
                            planner_reasoning = Some(r);
                        }
                    }
                    if !step_items.is_empty() {
                        break;
                    }
                }
            }
        }

        // 2. Fallback: Try to parse as JSON Array (Legacy Format)
        if step_items.is_empty() {
            let starts_arr: Vec<usize> = s.match_indices('[').map(|(i, _)| i).collect();
            for start in starts_arr {
                if let Some(end_offset) = s[start..].rfind(']') {
                    let end = start + end_offset;
                    let candidate = &s[start..=end];
                    if let Ok(items) = serde_json::from_str::<Vec<StepItem>>(candidate) {
                        step_items = items;
                        break;
                    }
                }
            }
        }
        
        let mut steps = Vec::new();
        for item in step_items {
            let n = item.tool;
            let deps = item.dependencies;
            let args: Value = Value::Null;
            let lower = n.to_lowercase();
            if lower == "memory" {
                steps.push(StepSpec::Memory);
            } else if lower == "profile" {
                steps.push(StepSpec::Profile);
            } else if lower == "relationship" {
                steps.push(StepSpec::Relationship);
            } else {
                let is_background = tools
                    .iter()
                    .find(|t| t.name == n)
                    .map(|t| t.is_long_running)
                    .unwrap_or(false);
                steps.push(StepSpec::Tool {
                    name: n,
                    args,
                    is_background,
                    dependencies: deps,
                });
            }
        }
        let plan = WorkflowPlan { steps, reasoning: planner_reasoning };
        info!("llm decision plan: {:?}", plan);
        Ok(plan)
    }
}
