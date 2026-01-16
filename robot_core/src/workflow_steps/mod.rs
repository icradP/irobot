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

        //打印tool和所需schema参数
        info!("LlmParameterResolver tool: {} \nSchema: {}", tool, schema_json);
        
        // Try to extract workflow context from memory
        let workflow_context = if let Some(workflow) = ctx.memory.get("workflow") {
            info!("LlmParameterResolver found workflow in memory: {:?}", workflow);
            if let (Some(plan), Some(current_idx_val)) = (workflow.get("plan"), workflow.get("current_step_index")) {
                 if let (Some(steps), Some(current_idx)) = (plan.get("steps").and_then(|s| s.as_array()), current_idx_val.as_u64()) {
                     let current_idx = current_idx as usize;
                     let mut s = String::from("\nWorkflow Context (You are resolving parameters for the CURRENT step):\n");

                     if let Some(reasoning) = plan.get("reasoning").and_then(|v| v.as_str()) {
                         if !reasoning.trim().is_empty() {
                             s.push_str(&format!("Planner Reasoning (Use this to understand intent):\n{}\n\n", reasoning));
                         }
                     }
                     
                     // Build execution history map
                     let history_map: std::collections::HashMap<usize, (Value, Option<Value>)> = if let Some(history) = workflow.get("history").and_then(|h| h.as_array()) {
                         history.iter().filter_map(|h| {
                             if let (Some(idx), Some(args)) = (h.get("step_index").and_then(|v| v.as_u64()), h.get("args")) {
                                 let result = h.get("result").cloned();
                                 Some((idx as usize, (args.clone(), result)))
                             } else {
                                 None
                             }
                         }).collect()
                     } else {
                         std::collections::HashMap::new()
                     };

                     for (i, step) in steps.iter().enumerate() {
                         let step_name = if let Some(t) = step.get("Tool") {
                             t.get("name").and_then(|n| n.as_str()).unwrap_or("Unknown Tool")
                         } else if step.as_str().is_some() {
                             "System Step"
                         } else {
                             "Unknown Step"
                         };
                         
                         let status = if i < current_idx {
                             if let Some((args, result)) = history_map.get(&i) {
                                 let result_str = if let Some(res) = result {
                                     format!(" -> Result: {}", res)
                                 } else {
                                     "".to_string()
                                 };
                                 format!("(Completed) - Executed with args: {}{}", args, result_str)
                             } else {
                                 "(Completed)".to_string()
                             }
                         } else if i == current_idx {
                             // Check dependencies
                             let mut dep_info = String::new();
                             if let Some(t) = step.get("Tool") {
                                 if let Some(deps) = t.get("dependencies").and_then(|d| d.as_array()) {
                                     let dep_indices: Vec<usize> = deps.iter().filter_map(|v| v.as_u64().map(|u| u as usize)).collect();
                                     if !dep_indices.is_empty() {
                                         dep_info = format!(" [Depends on Steps: {:?}]", dep_indices);
                                         
                                         // Append results from dependencies
                                         for &dep_idx in &dep_indices {
                                             if let Some((_, result)) = history_map.get(&dep_idx) {
                                                 if let Some(res) = result {
                                                     dep_info.push_str(&format!("\n    - Step {} Result: {}", dep_idx + 1, res));
                                                 }
                                             }
                                         }
                                     }
                                 }
                             }
                             format!("(CURRENT - FOCUS HERE){}", dep_info)
                         } else {
                             "(Pending)".to_string()
                         };
                         s.push_str(&format!("{}. {} {}\n", i + 1, step_name, status));
                     }
                     info!("LlmParameterResolver generated workflow context: {}", s);
                     Some(s)
                 } else {
                     info!("LlmParameterResolver failed to parse steps or current_idx");
                     None
                 }
            } else {
                info!("LlmParameterResolver failed to get plan or current_step_index");
                None
            }
        } else {
            info!("LlmParameterResolver did NOT find workflow in memory. Keys: {:?}", ctx.memory.as_object().map(|m| m.keys().collect::<Vec<_>>()));
            None
        };

        let system_prompt_suffix = workflow_context.clone().unwrap_or_default();

        let system = if schema_json.is_empty() {
            "Convert user's input to a JSON object of tool parameters. Respond with ONLY a valid JSON object.".to_string()
        } else {
            format!(
                "You are a strict parameter extractor. Your goal is to convert user input into a JSON object for a specific tool.\n\
                Tool Name: {}\n\
                Tool Description: {}\n\
                Parameter Schema: {}\n\
                Required Fields: {:?}\n\
                {}\n\
                Instructions:
                1. ONLY extract parameters that are explicitly mentioned or clearly implied in the user's input.
                2. For required fields:
                3. Return ONLY the JSON object. No markdown, no explanations.
                4. If a required field cannot be found in the input, use null - the system will prompt the user.
                5. Prioritize accuracy over trying to complete missing information.
                6. Use the Workflow Context to disambiguate parameters. 
                   - Review 'Completed' steps and their 'Executed with args' AND 'Result'.
                   - Do NOT reuse parameters from completed steps unless the user explicitly asks for the same thing again.
                   - Extract parameters ONLY for the CURRENT step, corresponding to the NEXT logical part of the user input that hasn't been processed yet.
                   - IMPORTANT: Infer dependencies dynamically:
                     * INDEPENDENT: If a step introduces NEW information, extract parameters from the original input.
                     * DEPENDENT (Single): If a step acts on a previous result, use the previous Result as input.
                     * DEPENDENT (Multi): If a step combines multiple previous results, use ALL relevant previous Results as inputs.
                   - EXPLICIT DEPENDENCIES: The current step explicitly depends on steps listed in 'Depends on Steps'. PRIORITY: Use results from these specific steps.",
                tool, description, schema_json, required_fields, system_prompt_suffix
            )
        };
        
        info!("LlmParameterResolver System Prompt:\n{}", system);

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
        
        let workflow_context_user = workflow_context.clone().unwrap_or_default();
        
        let user = if prev_str.is_empty() {
            format!("Input: {}\n{}\nReturn JSON:", input_text, workflow_context_user)
        } else {
            format!("Input: {}\n{}\n{}\nReturn JSON:", input_text, prev_str, workflow_context_user)
        };
        
        info!("LlmParameterResolver User Prompt:\n{}", user);
        
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
            session_id: ctx.session_id.clone(),
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

        // --- Parameter Evaluation and Correction Module ---
        let evaluator = ParameterEvaluator {
            llm: self.llm.clone(),
            model: self.model.clone(),
        };
        
        let fixed_v = evaluator.evaluate_and_fix(
            tool,
            &schema_json,
            &input_text,
            &v,
            &workflow_context_user,
            &required_fields
        ).await.unwrap_or_else(|e| {
            tracing::error!("Parameter evaluation failed, using original args: {}", e);
            v.clone()
        });
        
        let mut v = fixed_v;
        // ---------------------------------------------------

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

// Module for Parameter Evaluation
struct ParameterEvaluator {
    llm: Arc<dyn LLMClient + Send + Sync>,
    model: String,
}

impl ParameterEvaluator {
    async fn evaluate_and_fix(
        &self,
        tool: &str,
        schema: &str,
        user_input: &str,
        generated_args: &Value,
        context: &str,
        required_fields: &[String]
    ) -> anyhow::Result<Value> {
        let system = format!(
            "You are a Parameter Auditor. Your job is to verify and fix the arguments generated for a tool call.\n\
            Tool: {}\n\
            Schema: {}\n\
            Required Fields: {:?}\n\
            \n\
            Rules:\n\
            1. Check if the 'Generated Args' match the 'User Input' and 'Context'.\n\
            2. Check if the data types match the Schema (e.g. string vs number).\n\
            3. Check if all required fields are present and valid.\n\
            4. If the args are correct, return them exactly as is.\n\
            5. If there are errors (missing fields, wrong types, hallucinated values), FIX them.\n\
            6. Return ONLY the valid JSON object of the arguments. No markdown.",
            tool, schema, required_fields
        );

        let user = format!(
            "User Input: {}\n\
            Context: {}\n\
            Generated Args: {}\n\
            \n\
            Please evaluate and fix the arguments. Return JSON:",
            user_input, context, generated_args
        );

        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage { role: "system".into(), content: system },
                ChatMessage { role: "user".into(), content: user },
            ],
            temperature: Some(0.1),
            session_id: None, // Parameter evaluator is internal, maybe no need to show think? Or use ctx?
                              // But ParameterEvaluator struct doesn't have ctx access.
                              // If we want to show think, we need to pass session_id to evaluate_and_fix.
        };

        info!("ParameterEvaluator checking args: {}", generated_args);
        let out = self.llm.chat(req).await?;
        let s = out.text.trim();
        
        let json_slice = if let Some(start) = s.find('{') {
            if let Some(end) = s.rfind('}') {
                if end >= start { &s[start..=end] } else { s }
            } else {
                s
            }
        } else {
            s
        };

        let v: serde_json::Value = serde_json::from_str(json_slice)?;
        info!("ParameterEvaluator result: {}", v);
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
        if let Some(map) = ctx.memory.as_object_mut() {
            map.insert("input_text".to_string(), serde_json::Value::String(ctx.input_text.clone()));
            map.insert("touched".to_string(), serde_json::Value::Bool(true));
        } else {
            ctx.memory = serde_json::json!({"input_text": ctx.input_text, "touched": true});
        }
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

        // Record execution history (BEFORE calling, using resolved args)
        if let Some(workflow) = ctx.memory.get_mut("workflow") {
            if let Some(current_idx) = workflow.get("current_step_index").and_then(|v| v.as_u64()) {
                if let Some(workflow_obj) = workflow.as_object_mut() {
                     let history = workflow_obj.entry("history").or_insert_with(|| serde_json::Value::Array(Vec::new()));
                     if let Some(arr) = history.as_array_mut() {
                         arr.push(serde_json::json!({
                             "step_index": current_idx,
                             "tool": self.name,
                             "args": resolved_args
                         }));
                     }
                }
            }
        }

        let val = mcp.call(&self.name, resolved_args.clone()).await?;
        if let Some(map) = ctx.memory.as_object_mut() {
            map.insert("last_tool_result".to_string(), val.clone());
            
            // Update workflow history with result
            if let Some(workflow) = map.get_mut("workflow") {
                if let Some(workflow_obj) = workflow.as_object_mut() {
                    if let Some(history) = workflow_obj.get_mut("history").and_then(|h| h.as_array_mut()) {
                        if let Some(last_entry) = history.last_mut() {
                             if let Some(entry_obj) = last_entry.as_object_mut() {
                                 entry_obj.insert("result".to_string(), val.clone());
                             }
                        }
                    }
                }
            }
        } else {
            ctx.memory = serde_json::json!({"last_tool_result": val.clone()});
        }
        let o = OutputEvent {
            target: "default".into(),
            source: "system".into(),
            session_id: ctx.session_id.clone(),
            content: val,
            style: ctx.persona.style.clone(),
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
            dependencies: _,
        } => Box::new(McpToolStep {
            name: name.clone(),
            args: args.clone(),
            resolver,
        }),
    }
}
