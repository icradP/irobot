use crate::llm::adapter::{ChatMessage, ChatRequest, LLMClient};
use crate::mcp::client::MCPClient;
use crate::utils::{Context, OutputEvent, StepSpec};
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tracing::info;

#[derive(Clone, Debug)]
pub enum StepStatus {
    Continue,
    Stop,
    WaitUser(String), // Prompt for user
}

#[derive(Clone)]
pub struct StepResult {
    pub status: StepStatus,
    pub output: Option<OutputEvent>,
}

#[async_trait]
pub trait WorkflowStep: Send + Sync {
    async fn run(&self, ctx: &mut Context, mcp: &dyn MCPClient) -> anyhow::Result<StepResult>;
}

pub struct MemoryStep;
pub struct ProfileStep;
pub struct RelationshipStep;
pub struct McpToolStep {
    pub name: String,
    pub args: Value,
    pub resolver: Arc<dyn ParameterResolver + Send + Sync>,
}

#[async_trait]
pub trait ParameterResolver: Send + Sync {
    async fn resolve(
        &self,
        mcp: &dyn MCPClient,
        tool: &str,
        input: &Value,
        ctx: &Context,
    ) -> anyhow::Result<Value>;
}

pub struct NoopResolver;

#[async_trait]
impl ParameterResolver for NoopResolver {
    async fn resolve(
        &self,
        _mcp: &dyn MCPClient,
        _tool: &str,
        input: &Value,
        _ctx: &Context,
    ) -> anyhow::Result<Value> {
        Ok(input.clone())
    }
}

pub struct LlmParameterResolver {
    pub llm: Arc<dyn LLMClient + Send + Sync>,
    pub model: String,
}

#[async_trait]
impl ParameterResolver for LlmParameterResolver {
    async fn resolve(
        &self,
        mcp: &dyn MCPClient,
        tool: &str,
        input: &Value,
        ctx: &Context,
    ) -> anyhow::Result<Value> {
        if input.is_object() {
            return Ok(input.clone());
        }
        let schema = mcp.tool_schema(tool).await?;
        let schema_json = schema
            .as_ref()
            .map(|s| serde_json::to_string(s).unwrap_or_default())
            .unwrap_or_default();

        // Fetch tool description for better context
        let tools = mcp.list_tools().await.unwrap_or_default();
        let description = tools
            .iter()
            .find(|t| t.name == tool)
            .map(|t| t.description.clone())
            .unwrap_or_default();

        // Get required fields to ensure they are properly filled
        let required_fields = mcp.required_fields(tool).await.unwrap_or_default();

        let input_text = match input {
            Value::String(s) => s.clone(),
            Value::Null => ctx.input_text.clone(),
            v => v.to_string(),
        };

        let system = if schema_json.is_empty() {
            "Convert user's input to a JSON object of tool parameters. Respond with ONLY a valid JSON object.".to_string()
        } else {
            format!(
                "You are a strict parameter extractor. Your goal is to convert user input into a JSON object for a specific tool.\n\
                Tool Name: {}\n\
                Tool Description: {}\n\
                Parameter Schema: {}\n\
                Required Fields: {:?}\n\
                Instructions:\n\
                1. ONLY extract parameters that are explicitly mentioned or clearly implied in the user's input.\n\
                2. For required fields:\n\
                   - If the field value is CLEARLY stated in the input, extract it.\n\
                   - If the field value is NOT mentioned and CANNOT be reasonably inferred from context, use null.\n\
                   - NEVER guess or assume values (e.g., do not assume 'Beijing' just because it's a common city).\n\
                3. Return ONLY the JSON object. No markdown, no explanations.\n\
                4. If a required field cannot be found in the input, use null - the system will prompt the user.\n\
                5. Prioritize accuracy over trying to complete missing information.",
                tool, description, schema_json, required_fields
            )
        };

        //打印tool和所需schema参数
        info!("LlmParameterResolver tool: {} \nSchema: {}", tool, schema_json);
        
        let prev = ctx
            .memory
            .get("last_tool_result")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let prev_str = if prev.is_null() {
            String::new()
        } else {
            format!("Previous result: {}", prev)
        };
        let user = if prev_str.is_empty() {
            format!("Input: {}\nReturn JSON:", input_text)
        } else {
            format!("Input: {}\n{}\nReturn JSON:", input_text, prev_str)
        };
        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: system,
                },
                ChatMessage {
                    role: "user".into(),
                    content: user,
                },
            ],
            temperature: Some(0.1),
        };
        tracing::info!(
            "LlmParameterResolver calling LLM with input: {}",
            input_text
        );
        let out = self.llm.chat(req).await?;
        let s = out.text.trim();
        tracing::info!("LlmParameterResolver LLM output: {}", s);

        let json_slice = if let Some(start) = s.find('{') {
            if let Some(end) = s.rfind('}') {
                if end >= start { &s[start..=end] } else { s }
            } else {
                s
            }
        } else {
            s
        };
        let v: serde_json::Value = serde_json::from_str(json_slice).map_err(|e| {
            anyhow::anyhow!(
                "Failed to parse JSON from LLM output: '{}'. Error: {}",
                json_slice,
                e
            )
        })?;

        let mut v = v;
        normalize_null_strings(&mut v);

        // Merge with original input if it was an object (preserve Planner's explicit values)
        if let Some(original_obj) = input.as_object() {
            if let Some(v_obj) = v.as_object_mut() {
                for (k, val) in original_obj {
                    if !val.is_null() {
                        v_obj.insert(k.clone(), val.clone());
                    }
                }
            }
        }

        ensure_required_fields_present(&mut v, &required_fields);
        Ok(v)
    }
}

fn normalize_null_strings(v: &mut Value) {
    match v {
        Value::String(s) => {
            if s.trim().eq_ignore_ascii_case("null") {
                *v = Value::Null;
            }
        }
        Value::Array(arr) => {
            for it in arr {
                normalize_null_strings(it);
            }
        }
        Value::Object(map) => {
            for (_k, val) in map.iter_mut() {
                normalize_null_strings(val);
            }
        }
        _ => {}
    }
}

fn ensure_required_fields_present(v: &mut Value, required_fields: &[String]) {
    let Some(obj) = v.as_object_mut() else {
        return;
    };
    for field in required_fields {
        match obj.get(field) {
            Some(val) if !val.is_null() => {}
            _ => {
                obj.insert(field.clone(), Value::Null);
            }
        }
    }
}

#[async_trait]
impl WorkflowStep for MemoryStep {
    async fn run(&self, ctx: &mut Context, _mcp: &dyn MCPClient) -> anyhow::Result<StepResult> {
        info!("step memory run");
        ctx.memory = serde_json::json!({"input_text": ctx.input_text, "touched": true});
        Ok(StepResult {
            status: StepStatus::Continue,
            output: None,
        })
    }
}

#[async_trait]
impl WorkflowStep for ProfileStep {
    async fn run(&self, ctx: &mut Context, _mcp: &dyn MCPClient) -> anyhow::Result<StepResult> {
        info!("step profile run");
        ctx.touch_profile();
        Ok(StepResult {
            status: StepStatus::Continue,
            output: None,
        })
    }
}

#[async_trait]
impl WorkflowStep for RelationshipStep {
    async fn run(&self, ctx: &mut Context, _mcp: &dyn MCPClient) -> anyhow::Result<StepResult> {
        ctx.touch_relationships();
        let o = OutputEvent::from_context(ctx);
        Ok(StepResult {
            status: StepStatus::Stop,
            output: Some(o),
        })
    }
}

#[async_trait]
impl WorkflowStep for McpToolStep {
    async fn run(&self, ctx: &mut Context, mcp: &dyn MCPClient) -> anyhow::Result<StepResult> {
        let mut resolved_args = self
            .resolver
            .resolve(mcp, &self.name, &self.args, ctx)
            .await?;

        if let Some(session_id) = ctx.session_id.clone() {
            if let Some(obj) = resolved_args.as_object_mut() {
                if !obj.contains_key("session_id") {
                    obj.insert(
                        "session_id".to_string(),
                        serde_json::Value::String(session_id),
                    );
                }
            }
        }

        // Removed client-side validation to allow MCP server to handle elicitation

        let val = mcp.call(&self.name, resolved_args).await?;
        ctx.memory = serde_json::json!({"last_tool_result": val.clone()});
        let o = OutputEvent {
            target: "default".into(),
            source: "system".into(),
            session_id: ctx.session_id.clone(),
            content: val,
            style: crate::core::persona::OutputStyle::Neutral,
        };
        Ok(StepResult {
            status: StepStatus::Continue,
            output: Some(o),
        })
    }
}

pub fn build_step(
    spec: &StepSpec,
    resolver: Arc<dyn ParameterResolver + Send + Sync>,
) -> Box<dyn WorkflowStep> {
    match spec {
        StepSpec::Memory => Box::new(MemoryStep),
        StepSpec::Profile => Box::new(ProfileStep),
        StepSpec::Relationship => Box::new(RelationshipStep),
        StepSpec::Tool {
            name,
            args,
            is_background: _,
        } => Box::new(McpToolStep {
            name: name.clone(),
            args: args.clone(),
            resolver,
        }),
    }
}
