use crate::mcp::client::MCPClient;
use crate::mcp::registry::ToolMeta;
use crate::llm::adapter::{ChatMessage, ChatRequest, LLMClient};
use crate::utils::{OutputEvent, output_bus};
use crate::core::persona::OutputStyle;
use async_trait::async_trait;
    use rmcp::{
        model::{
            CallToolRequestParam, ClientCapabilities, ClientInfo, CreateElicitationRequestParam,
            CreateElicitationResult, ElicitationAction, Implementation,
        },
        service::{RequestContext, RoleClient, RunningService},
        ServiceExt,
    };
    use std::sync::{Arc, Mutex};

pub struct RmcpStdIoClient {
    service: RunningService<RoleClient, RobotClientHandler>,
    shared: Arc<Mutex<SharedCtx>>,
}

pub struct RobotClientHandler {
    info: ClientInfo,
    shared: Arc<Mutex<SharedCtx>>,

    llm: Arc<dyn LLMClient + Send + Sync>,
    model: String,
    session_id: String,
}

#[derive(Default)]
pub struct SharedCtx {
    pub last_result: Option<serde_json::Value>,
    pub last_elicitation_message: Option<String>,
    pub last_elicitation_schema: Option<serde_json::Value>,
    pub preview_only: bool,
}

impl rmcp::handler::client::ClientHandler for RobotClientHandler {
    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequestParam,
        _context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<CreateElicitationResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let schema_str = serde_json::to_string_pretty(&request.requested_schema).unwrap_or_default();
            eprintln!("Message: {}", request.message);
            eprintln!("Schema: {}", schema_str);
            eprintln!("Please provide input (Natural language or JSON): ");

            // Record elicitation prompt and schema for potential introspection
            {
                let mut guard = self.shared.lock().unwrap();
                guard.last_elicitation_message = Some(request.message.clone());
                guard.last_elicitation_schema = Some(
                    serde_json::to_value(&request.requested_schema)
                        .unwrap_or(serde_json::Value::Null),
                );
            }

            
            // Use the session_id bound to this client
            let sid = self.session_id.clone();

            // Emit output event to prompt the user
            let output_event = OutputEvent {
                target: "default".into(),
                source: "mcp".into(),
                session_id: Some(sid.clone()),
                content: serde_json::json!({
                    "message": request.message,
                    "schema": request.requested_schema
                }),
                style: OutputStyle::Neutral,
            };
            
            if let Err(e) = output_bus().send(output_event) {
                eprintln!("[elicit] Failed to send output event: {}", e);
            }

            let mut rx = crate::utils::event_bus().subscribe();
            let input_event = loop {
                match rx.recv().await {
                    Ok(ev) => {
                        let ev_sid = ev.session_id.clone().unwrap_or_else(|| ev.source.clone());
                        if ev_sid == sid {
                            break ev;
                        }
                    }
                    Err(_) => {
                        continue;
                    }
                }
            };
            let input = if let Some(s) = input_event
                .payload
                .get("content")
                .and_then(|v| v.as_str())
            {
                s.to_string()
            } else {
                input_event.payload.to_string()
            };

            // Try to parse as JSON first
            let parsed: serde_json::Value = match serde_json::from_str(&input) {
                Ok(v) => {
                    eprintln!("[elicit] Successfully parsed as direct JSON: {:?}", v);
                    // Mark event as consumed so core doesn't process it again
                    crate::utils::mark_event_consumed(input_event.id);
                    v
                }
                Err(_) => {
                    eprintln!("[elicit] Input is not valid JSON, attempting to use LLM to transform...");
                    
                    let system_prompt = format!(
                        "You are a helpful assistant that converts natural language input into a JSON object based on a provided schema.\n\
                        Schema:\n{}\n\
                        Context Message: {}\n\
                        \n\
                        Instructions:\n\
                        1. Analyze the user's natural language input.\n\
                        2. Map the input to the fields in the JSON schema.\n\
                        3. If a field is missing in the input but required by the schema, use null.\n\
                        4. Return ONLY the valid JSON object. Do not include markdown formatting (like ```json ... ```) or any explanations.\n",
                        schema_str, request.message
                    );

                    let req = ChatRequest {
                        model: self.model.clone(),
                        messages: vec![
                            ChatMessage {
                                role: "system".into(),
                                content: system_prompt,
                            },
                            ChatMessage {
                                role: "user".into(),
                                content: input.clone(),
                            },
                        ],
                        temperature: Some(0.1),
                    };

                    match self.llm.chat(req).await {
                        Ok(response) => {
                            let text = response.text.trim();
                            eprintln!("[elicit] LLM response: {}", text);
                            
                            // Clean up potential markdown code blocks
                            let json_str = if let Some(start) = text.find('{') {
                                if let Some(end) = text.rfind('}') {
                                    if end >= start {
                                        &text[start..=end]
                                    } else {
                                        text
                                    }
                                } else {
                                    text
                                }
                            } else {
                                text
                            };

                            match serde_json::from_str(json_str) {
                                Ok(v) => {
                                    // Mark event as consumed so core doesn't process it again
                                    crate::utils::mark_event_consumed(input_event.id);
                                    v
                                },
                                Err(e) => {
                                    eprintln!("[elicit] ERROR: LLM produced invalid JSON: {}", e);
                                    return Err(rmcp::ErrorData::invalid_params(
                                        format!("Failed to parse LLM output as JSON: {}", e),
                                        None,
                                    ));
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[elicit] ERROR: LLM call failed: {}", e);
                            return Err(rmcp::ErrorData::internal_error(
                                format!("LLM transformation failed: {}", e),
                                None,
                            ));
                        }
                    }
                }
            };

            eprintln!("[elicit] ✓ Parsed and returning to server\n");
            Ok(CreateElicitationResult {
                action: ElicitationAction::Accept,
                content: Some(parsed),
            })
        }
    }
}

impl RmcpStdIoClient {
    pub async fn new(
        llm: Arc<dyn LLMClient + Send + Sync>,
        model: String,
        session_id: String,
    ) -> anyhow::Result<Self> {
        // TCP connection to MCP server
        let server_addr =
            std::env::var("ROBOT_MCP_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:9001".to_string());

        tracing::info!("Connecting to MCP server at: {} for session {}", server_addr, session_id);
        let stream = tokio::net::TcpStream::connect(&server_addr).await?;
        tracing::info!("Connected to MCP server for session {}", session_id);

        let shared = Arc::new(Mutex::new(SharedCtx::default()));
        let client_info = ClientInfo {
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::builder().enable_elicitation().build(),
            client_info: Implementation {
                name: format!("robot-core-client-{}", session_id),
                title: None,
                version: "0.1.0".to_string(),
                website_url: None,
                icons: None,
            },
        };
        let handler = RobotClientHandler {
            info: client_info,
            shared: shared.clone(),

            llm,
            model,
            session_id,
        };
        let service = handler.serve(stream).await?;
        Ok(Self { service, shared })
    }
}

#[async_trait]
impl MCPClient for RmcpStdIoClient {
    async fn call(&self, tool: &str, args: serde_json::Value) -> anyhow::Result<serde_json::Value> {
        tracing::info!(
            "RmcpStdIoClient calling tool '{}' with args: {:?}",
            tool,
            args
        );

        // Check if arguments contain null values - if so, don't pass them to trigger elicitation
        let should_elicit = if let serde_json::Value::Object(ref obj) = args {
            obj.values().any(|v| v.is_null())
        } else {
            false
        };

        // Note: We no longer need to capture session_id from args because this client 
        // is already dedicated to a specific session_id.

        let arguments = if should_elicit {
            tracing::info!(
                "RmcpStdIoClient: arguments contain null values for tool '{}', passing None to trigger elicitation",
                tool
            );
            None
        } else if args.is_object() {
            Some(rmcp::model::object(args))
        } else {
            tracing::warn!(
                "RmcpStdIoClient args for tool '{}' is not an object, sending None. Args: {:?}",
                tool,
                args
            );
            None
        };

        let result = self
            .service
            .call_tool(CallToolRequestParam {
                name: tool.to_string().into(),
                arguments,
            })
            .await?;
        let val = serde_json::to_value(&result)?;
        {
            let mut guard = self.shared.lock().unwrap();
            guard.last_result = Some(val.clone());
        }
        Ok(val)
    }

    async fn list_tools(&self) -> anyhow::Result<Vec<ToolMeta>> {
        let tools = self.service.list_all_tools().await?;
        let metas = tools
            .into_iter()
            .map(|t| ToolMeta {
                name: t.name.to_string(),
                description: t.description.unwrap_or_default().to_string(),
            })
            .collect();
        Ok(metas)
    }

    async fn required_fields(&self, tool: &str) -> anyhow::Result<Vec<String>> {
        let tools = self.service.list_all_tools().await?;
        for t in tools {
            if t.name.as_ref() == tool {
                let schema = &*t.input_schema;
                if let Some(req) = schema.get("required").and_then(|v| v.as_array()) {
                    let fields: Vec<String> = req
                        .iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect();
                    return Ok(fields);
                }
            }
        }
        Ok(Vec::new())
    }

    async fn tool_schema(&self, tool: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let tools = self.service.list_all_tools().await?;
        for t in tools {
            if t.name.as_ref() == tool {
                return Ok(Some(serde_json::Value::Object((*t.input_schema).clone())));
            }
        }
        Ok(None)
    }

    async fn elicit_preview(&self, tool: &str) -> anyhow::Result<Option<serde_json::Value>> {
        // Build preview based on tool schema without triggering a server call
        let tools = self.service.list_all_tools().await?;
        for t in tools {
            if t.name.as_ref() == tool {
                let schema = serde_json::Value::Object((*t.input_schema).clone());
                let msg: std::borrow::Cow<'_, str> = t
                    .description
                    .clone()
                    .unwrap_or_else(|| std::borrow::Cow::Owned("请根据如下 schema 提供参数".to_string()));
                let payload = serde_json::json!({
                    "type": "elicitation",
                    "message": msg,
                    "schema": schema,
                });
                return Ok(Some(payload));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::RmcpStdIoClient;

    use crate::mcp::client::MCPClient;
    use crate::mcp::registry::ToolMeta;
    use crate::llm::adapter::{LLMClient, ChatRequest, ChatResponse};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockLLM;
    #[async_trait]
    impl LLMClient for MockLLM {
        async fn chat(&self, _request: ChatRequest) -> anyhow::Result<ChatResponse> {
            Ok(ChatResponse {
                text: "{}".to_string(),
                usage: None,
            })
        }
        async fn embedding(&self, _request: crate::llm::adapter::EmbeddingRequest) -> anyhow::Result<crate::llm::adapter::EmbeddingResponse> {
            Ok(crate::llm::adapter::EmbeddingResponse {
                data: vec![],
                usage: None,
            })
        }
    }

    #[tokio::test]
    async fn list_and_call_echo() {
        let mock_llm = Arc::new(MockLLM);
        let client = RmcpStdIoClient::new(mock_llm, "test-model".to_string()).await.unwrap();
        let tools: Vec<ToolMeta> = client.list_tools().await.unwrap();
        assert!(tools.iter().any(|t| t.name == "echo"));
        let args = serde_json::json!({"message": "hello"});
        let result: serde_json::Value = client.call("echo", args).await.unwrap();
        let text = result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(text, "hello");
    }
}
