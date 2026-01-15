use crate::core::persona::OutputStyle;
use crate::llm::adapter::{ChatMessage, ChatRequest, LLMClient};
use crate::mcp::client::MCPClient;
use crate::mcp::registry::ToolMeta;
use crate::utils::{OutputEvent, output_bus};
use async_trait::async_trait;
use futures::future::BoxFuture;
use rmcp::{
    ServiceExt,
    model::{
        CallToolRequest, CallToolRequestParam, CancelledNotificationParam, ClientCapabilities,
        ClientInfo, ClientRequest, CreateElicitationRequestParam, CreateElicitationResult,
        ElicitationAction, Implementation, ListRootsResult, RequestId, Root, ServerResult,
        ProgressNotificationParam,
    },
    service::{NotificationContext, PeerRequestOptions, RequestContext, RoleClient, RunningService, ServiceError},
};
use std::sync::{Arc, Mutex};

pub struct RmcpStdIoClient {
    server_addr: String,
    llm: Arc<dyn LLMClient + Send + Sync>,
    model: String,
    session_id: String,
    service: tokio::sync::Mutex<Option<RunningService<RoleClient, RobotClientHandler>>>,
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
    pub current_call_tool_request_id: Option<RequestId>,
}

fn is_cancel_text(s: &str) -> bool {
    let t = s.trim().to_ascii_lowercase();
    if t.is_empty() {
        return false;
    }
    let cancel_words = [
        "算了",
        "不用了",
        "取消",
        "停止",
        "不需要了",
        "stop",
        "cancel",
        "never mind",
        "nevermind",
        "quit",
        "exit",
    ];
    cancel_words.iter().any(|w| t.contains(w))
}

impl rmcp::handler::client::ClientHandler for RobotClientHandler {
    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }

    fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) -> impl std::future::Future<Output = ()> + Send + '_ {
        async move {
            let output_event = OutputEvent {
                target: "default".into(),
                source: "mcp".into(),
                session_id: Some(self.session_id.clone()),
                content: serde_json::json!({
                    "type": "progress",
                    "token": params.progress_token,
                    "progress": params.progress,
                    "total": params.total,
                    "message": params.message
                }),
                style: OutputStyle::Neutral,
            };
            if let Err(e) = output_bus().send(output_event) {
                eprintln!("[mcp_client] Failed to send progress output event: {}", e);
            }
        }
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequestParam,
        context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<CreateElicitationResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let schema_str =
                serde_json::to_string_pretty(&request.requested_schema).unwrap_or_default();
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
            let input = if let Some(s) = input_event.payload.get("content").and_then(|v| v.as_str())
            {
                s.to_string()
            } else {
                input_event.payload.to_string()
            };

            if is_cancel_text(&input) {
                crate::utils::mark_event_consumed(input_event.id);
                let output_event = OutputEvent {
                    target: "default".into(),
                    source: "mcp".into(),
                    session_id: Some(sid.clone()),
                    content: serde_json::json!({
                        "type": "tool_cancel",
                        "message": "已取消本次工具调用"
                    }),
                    style: OutputStyle::Neutral,
                };
                let _ = output_bus().send(output_event);
                let request_id = {
                    let guard = self.shared.lock().unwrap();
                    guard.current_call_tool_request_id.clone()
                };
                if let Some(request_id) = request_id {
                    let _ = context
                        .peer
                        .notify_cancelled(CancelledNotificationParam {
                            request_id,
                            reason: Some("user cancelled".to_string()),
                        })
                        .await;
                }
                return Ok(CreateElicitationResult {
                    action: ElicitationAction::Cancel,
                    content: None,
                });
            }

            // Try to parse as JSON first
            let parsed: serde_json::Value = match serde_json::from_str(&input) {
                Ok(v) => {
                    eprintln!("[elicit] Successfully parsed as direct JSON: {:?}", v);
                    // Mark event as consumed so core doesn't process it again
                    crate::utils::mark_event_consumed(input_event.id);
                    v
                }
                Err(_) => {
                    eprintln!(
                        "[elicit] Input is not valid JSON, attempting to use LLM to transform..."
                    );

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
                                }
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

    fn list_roots(
        &self,
        _context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<ListRootsResult, rmcp::ErrorData>> + Send + '_
    {
        async move {
            let current_dir = std::env::current_dir().map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Failed to get current dir: {}", e), None)
            })?;

            Ok(ListRootsResult {
                roots: vec![Root {
                    uri: format!("file://{}", current_dir.display()).into(),
                    name: Some("Current Working Directory".into()),
                }],
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
        let server_addr =
            std::env::var("ROBOT_MCP_SERVER_ADDR").unwrap_or_else(|_| "127.0.0.1:9001".to_string());
        let shared = Arc::new(Mutex::new(SharedCtx::default()));
        Ok(Self {
            server_addr,
            llm,
            model,
            session_id,
            service: tokio::sync::Mutex::new(None),
            shared,
        })
    }

    async fn connect(&self) -> anyhow::Result<RunningService<RoleClient, RobotClientHandler>> {
        tracing::info!(
            "Connecting to MCP server at: {} for session {}",
            self.server_addr,
            self.session_id
        );
        let stream = tokio::net::TcpStream::connect(&self.server_addr).await?;
        tracing::info!("Connected to MCP server for session {}", self.session_id);

        let client_info = ClientInfo {
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::builder()
                .enable_elicitation()
                .enable_roots()
                .build(),
            client_info: Implementation {
                name: format!("robot-core-client-{}", self.session_id),
                title: None,
                version: "0.1.0".to_string(),
                website_url: None,
                icons: None,
            },
        };
        let handler = RobotClientHandler {
            info: client_info,
            shared: self.shared.clone(),
            llm: self.llm.clone(),
            model: self.model.clone(),
            session_id: self.session_id.clone(),
        };
        Ok(handler.serve(stream).await?)
    }

    async fn connect_new(&self) -> anyhow::Result<RunningService<RoleClient, RobotClientHandler>> {
        tracing::info!(
            "Creating NEW connection to MCP server at: {} for session {}",
            self.server_addr,
            self.session_id
        );
        let stream = tokio::net::TcpStream::connect(&self.server_addr).await?;
        
        let client_info = ClientInfo {
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::builder()
                .enable_elicitation()
                .enable_roots()
                .build(),
            client_info: Implementation {
                name: format!("robot-core-client-{}-bg", self.session_id),
                title: None,
                version: "0.1.0".to_string(),
                website_url: None,
                icons: None,
            },
        };
        let handler = RobotClientHandler {
            info: client_info,
            shared: self.shared.clone(),
            llm: self.llm.clone(),
            model: self.model.clone(),
            session_id: self.session_id.clone(),
        };
        Ok(handler.serve(stream).await?)
    }

    async fn ensure_connected(&self) -> anyhow::Result<()> {
        {
            let guard = self.service.lock().await;
            if guard.is_some() {
                return Ok(());
            }
        }

        let service = self.connect().await?;
        let mut guard = self.service.lock().await;
        if guard.is_none() {
            *guard = Some(service);
        }
        Ok(())
    }

    fn should_reconnect(error_text: &str) -> bool {
        let s = error_text.to_ascii_lowercase();
        s.contains("broken pipe")
            || s.contains("connection")
            || s.contains("transport")
            || s.contains("closed")
            || s.contains("eof")
            || s.contains("reset by peer")
            || s.contains("os error")
    }

    async fn with_service_retry<T, E, F>(&self, op_name: &'static str, f: F) -> anyhow::Result<T>
    where
        F: for<'a> Fn(
                &'a RunningService<RoleClient, RobotClientHandler>,
            ) -> BoxFuture<'a, Result<T, E>>
            + Clone
            + Send,
        E: std::error::Error + Send + Sync + 'static,
        T: Send,
    {
        self.ensure_connected().await?;

        let f1 = f.clone();
        let res = {
            let guard = self.service.lock().await;
            let service = guard
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("MCP 未连接"))?;
            f1(service).await
        };

        match res {
            Ok(v) => Ok(v),
            Err(e) => {
                let text = e.to_string();
                if Self::should_reconnect(&text) {
                    tracing::warn!("MCP {} 失败，尝试重连: {}", op_name, text);
                    {
                        let mut guard = self.service.lock().await;
                        *guard = None;
                    }
                    self.ensure_connected().await?;
                    let f2 = f.clone();
                    let res2 = {
                        let guard = self.service.lock().await;
                        let service = guard
                            .as_ref()
                            .ok_or_else(|| anyhow::anyhow!("MCP 未连接"))?;
                        f2(service).await
                    };
                    Ok(res2?)
                } else {
                    Err(anyhow::anyhow!(e))
                }
            }
        }
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

        // Only elicit when REQUIRED fields are missing/empty
        let required = self.required_fields(tool).await.unwrap_or_default();
        let mut missing_fields: Vec<String> = Vec::new();
        if let serde_json::Value::Object(ref obj) = args {
            for f in &required {
                match obj.get(f) {
                    None => missing_fields.push(f.clone()),
                    Some(v) => {
                        let is_missing = match v {
                            serde_json::Value::Null => true,
                            serde_json::Value::String(s) => {
                                let t = s.trim();
                                t.is_empty() || t.eq_ignore_ascii_case("null")
                            }
                            serde_json::Value::Array(a) => a.is_empty(),
                            _ => false,
                        };
                        if is_missing {
                            missing_fields.push(f.clone());
                        }
                    }
                }
            }
        }
        let should_elicit = !missing_fields.is_empty();

        // Note: We no longer need to capture session_id from args because this client
        // is already dedicated to a specific session_id.

        let arguments = if should_elicit {
            tracing::info!(
                "RmcpStdIoClient: required fields missing for tool '{}': {:?}. Passing None to trigger elicitation",
                tool, missing_fields
            );
            if let serde_json::Value::Object(obj) = &args {
                let mut meta = serde_json::Map::new();
                for (k, v) in obj.iter() {
                    if k == "session_id" || k.starts_with("__") {
                        meta.insert(k.clone(), v.clone());
                    }
                }
                if meta.is_empty() {
                    None
                } else {
                    Some(rmcp::model::object(serde_json::Value::Object(meta)))
                }
            } else {
                None
            }
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

        let tool_name = tool.to_string();

        // Check if this is a background/parallel task (marked by metadata)
        let is_parallel = {
            let tools = self.list_tools().await.unwrap_or_default();
            tools.iter().any(|t| t.name == tool && t.is_long_running)
        };

        if is_parallel {
            tracing::info!("RmcpStdIoClient: Detected parallel tool '{}', creating dedicated connection", tool);
            let service = self.connect_new().await?;
            let request = ClientRequest::CallToolRequest(CallToolRequest {
                method: Default::default(),
                params: CallToolRequestParam {
                    name: tool_name.clone().into(),
                    arguments: arguments.clone(),
                },
                extensions: Default::default(),
            });
            // Parallel execution does not use shared lock for request ID tracking
            let handle = service
                .send_cancellable_request(request, PeerRequestOptions::no_options())
                .await?;
            
            let response = match handle.await_response().await {
                Ok(r) => r,
                Err(ServiceError::Cancelled { reason }) => {
                    let msg = format!(
                        "tool_cancel\nname={}\nmessage={}",
                        tool_name,
                        reason.unwrap_or_else(|| "用户取消了本次工具调用".to_string())
                    );
                    return Ok(serde_json::to_value(rmcp::model::CallToolResult::success(vec![
                        rmcp::model::Content::text(msg),
                    ]))?);
                }
                Err(e) => return Err(anyhow::anyhow!(e)),
            };

            let result = match response {
                ServerResult::CallToolResult(r) => r,
                _ => return Err(anyhow::anyhow!("Unexpected response")),
            };
            return Ok(serde_json::to_value(result)?);
        }

        let shared = self.shared.clone();
        let tool_name_clone = tool_name.clone();
        let result: rmcp::model::CallToolResult = match self
            .with_service_retry("call_tool", move |svc| {
                let arguments = arguments.clone();
                let tool_name = tool_name_clone.clone();
                let shared = shared.clone();
                Box::pin(async move {
                    let request = ClientRequest::CallToolRequest(CallToolRequest {
                        method: Default::default(),
                        params: CallToolRequestParam {
                            name: tool_name.clone().into(),
                            arguments,
                        },
                        extensions: Default::default(),
                    });

                    let handle = svc
                        .send_cancellable_request(request, PeerRequestOptions::no_options())
                        .await?;
                    {
                        let mut guard = shared.lock().unwrap();
                        guard.current_call_tool_request_id = Some(handle.id.clone());
                    }

                    let response = match handle.await_response().await {
                        Ok(r) => r,
                        Err(ServiceError::Cancelled { reason }) => {
                            let msg = format!(
                                "tool_cancel\nname={}\nmessage={}",
                                tool_name,
                                reason.unwrap_or_else(|| "用户取消了本次工具调用".to_string())
                            );
                            return Ok(rmcp::model::CallToolResult::success(vec![
                                rmcp::model::Content::text(msg),
                            ]));
                        }
                        Err(e) => return Err(e),
                    };

                    match response {
                        ServerResult::CallToolResult(r) => Ok(r),
                        _ => Err(ServiceError::UnexpectedResponse),
                    }
                })
            })
            .await
        {
            Ok(r) => r,
            Err(e) => {
                {
                    let mut guard = self.shared.lock().unwrap();
                    guard.current_call_tool_request_id = None;
                }
                return Err(e);
            }
        };
        {
            let mut guard = self.shared.lock().unwrap();
            guard.current_call_tool_request_id = None;
        }
        let val = serde_json::to_value(&result)?;
        {
            let mut guard = self.shared.lock().unwrap();
            guard.last_result = Some(val.clone());
        }
        Ok(val)
    }

    async fn list_tools(&self) -> anyhow::Result<Vec<ToolMeta>> {
        let tools = self
            .with_service_retry("list_tools", |svc| Box::pin(svc.list_all_tools()))
            .await?;
        let metas = tools
            .into_iter()
            .map(|t| {
                let description = t.description.unwrap_or_default().to_string();
                let is_long_running = t
                    .meta
                    .as_ref()
                    .and_then(|m| m.get("isLongRunning"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                ToolMeta {
                    name: t.name.to_string(),
                    description,
                    is_long_running,
                }
            })
            .collect();
        Ok(metas)
    }

    async fn required_fields(&self, tool: &str) -> anyhow::Result<Vec<String>> {
        let tools = self
            .with_service_retry("required_fields(list_tools)", |svc| {
                Box::pin(svc.list_all_tools())
            })
            .await?;
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
        let tools = self
            .with_service_retry("tool_schema(list_tools)", |svc| {
                Box::pin(svc.list_all_tools())
            })
            .await?;
        for t in tools {
            if t.name.as_ref() == tool {
                return Ok(Some(serde_json::Value::Object((*t.input_schema).clone())));
            }
        }
        Ok(None)
    }

    async fn elicit_preview(&self, tool: &str) -> anyhow::Result<Option<serde_json::Value>> {
        let tools = self
            .with_service_retry("elicit_preview(list_tools)", |svc| {
                Box::pin(svc.list_all_tools())
            })
            .await?;
        for t in tools {
            if t.name.as_ref() == tool {
                let schema = serde_json::Value::Object((*t.input_schema).clone());
                let msg: std::borrow::Cow<'_, str> = t.description.clone().unwrap_or_else(|| {
                    std::borrow::Cow::Owned("请根据如下 schema 提供参数".to_string())
                });
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

    use crate::llm::adapter::{ChatOutput, ChatRequest, LLMClient};
    use crate::mcp::client::MCPClient;
    use crate::mcp::registry::ToolMeta;
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockLLM;
    #[async_trait]
    impl LLMClient for MockLLM {
        async fn chat(&self, _request: ChatRequest) -> anyhow::Result<ChatOutput> {
            Ok(ChatOutput {
                text: "{}".to_string(),
                raw: serde_json::Value::Object(serde_json::Map::new()),
            })
        }
    }

    #[tokio::test]
    #[ignore]
    async fn list_and_call_echo() {
        let mock_llm = Arc::new(MockLLM);
        let client = RmcpStdIoClient::new(
            mock_llm,
            "test-model".to_string(),
            "test-session".to_string(),
        )
        .await
        .unwrap();
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
