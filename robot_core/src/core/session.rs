use crate::core::decision_engine::{DecisionEngine, LLMDecisionEngine};
use crate::core::intent::{IntentDecision, IntentModule};
use crate::core::output_handler::OutputHandler;
use crate::core::perception::PerceptionModule;
use crate::core::persona::{OutputStyle, Persona};
use crate::core::router::{EventRouter, HandlerId};
use crate::core::sessions::web_session::WebSession;
use crate::core::tasks::client::TaskAwareMcpClient;
use crate::core::tasks::manager::TaskManager;
use crate::core::workflow_engine::WorkflowEngine;
use crate::mcp::client::MCPClient;
use crate::utils::{InputEvent, OutputEvent, StepSpec, Context};
use async_trait::async_trait;
use futures::future::join_all;
use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;
use crate::workflow_steps::StepStatus;

pub enum SessionMessage {
    Input(InputEvent),
    Shutdown,
}

#[async_trait]
pub trait SessionActor: Send + Sync {
    async fn run(self: Box<Self>);
}

pub struct RobotSession {
    pub id: String,
    pub inbox: mpsc::UnboundedReceiver<SessionMessage>,
    pub mcp_client: Arc<dyn MCPClient + Send + Sync>,
    pub decision_engine: Arc<Box<dyn DecisionEngine + Send + Sync>>,
    pub workflow_engine: Arc<WorkflowEngine>,
    pub perception_module: Arc<Box<dyn PerceptionModule + Send + Sync>>,
    pub intent_module: Arc<Box<dyn IntentModule + Send + Sync>>,
    pub persona: Arc<Persona>,
    pub output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>>,
    pub router: Arc<StdRwLock<EventRouter>>,
    pub task_manager: Arc<TaskManager>,
    // State for pending execution (WaitUser)
    pub pending_execution: Option<(Vec<StepSpec>, usize, Context)>,
}

#[async_trait]
impl SessionActor for RobotSession {
    async fn run(self: Box<Self>) {
        (*self).run_inner().await;
    }
}

impl RobotSession {
    pub fn new(
        id: String,
        mcp_client: Arc<dyn MCPClient + Send + Sync>,
        decision_engine: Arc<Box<dyn DecisionEngine + Send + Sync>>,
        workflow_engine: Arc<WorkflowEngine>,
        perception_module: Arc<Box<dyn PerceptionModule + Send + Sync>>,
        intent_module: Arc<Box<dyn IntentModule + Send + Sync>>,
        persona: Arc<Persona>,
        output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>>,
        router: Arc<StdRwLock<EventRouter>>,
        inbox: mpsc::UnboundedReceiver<SessionMessage>,
    ) -> Self {
        let task_manager = Arc::new(TaskManager::new());
        let aware_client = Arc::new(TaskAwareMcpClient::new(mcp_client, task_manager.clone()));

        Self {
            id,
            inbox,
            mcp_client: aware_client,
            decision_engine,
            workflow_engine,
            perception_module,
            intent_module,
            persona,
            output_handlers,
            router,
            task_manager,
            pending_execution: None,
        }
    }

    pub async fn run_inner(mut self) {
        info!("Session {} started", self.id);
        while let Some(msg) = self.inbox.recv().await {
            match msg {
                SessionMessage::Input(event) => {
                    self.handle_input(event).await;
                }
                SessionMessage::Shutdown => {
                    info!("Session {} shutting down", self.id);
                    break;
                }
            }
        }
    }

    async fn handle_input(&mut self, event: InputEvent) {
        info!("Session {} processing event from {}", self.id, event.source);

        // check if consumed
        if crate::utils::check_and_remove_consumed_event(&event.id) {
            info!(
                "Skipping event {} as it was consumed by MCP elicitation",
                event.id
            );
            return;
        }
        
        let sid_key = event
            .session_id
            .clone()
            .unwrap_or_else(|| event.source.clone());
        if crate::utils::is_elicitation_active(&sid_key) {
            info!(
                "Session {} skipping event {} because MCP elicitation is active",
                self.id, event.id
            );
            return;
        }
        
        // 0. Routing logic (pre-calculation for later use)
        let target_ids = {
            let route_ids_opt = {
                let router = self.router.read().unwrap();
                if router.has_routes() {
                    Some(router.get_outputs_for_event(&event))
                } else {
                    None
                }
            };

            match route_ids_opt {
                Some(ids) if !ids.is_empty() => ids,
                _ => self.output_handlers.read().await.keys().cloned().collect(),
            }
        };

        let input_text = if let Some(line) = event
            .payload
            .get("line")
            .and_then(|v: &serde_json::Value| v.as_str())
        {
            line.to_string()
        } else if let Some(content) = event
            .payload
            .get("content")
            .and_then(|v: &serde_json::Value| v.as_str())
        {
            content.to_string()
        } else {
            String::new()
        };

        // Check for pending execution (Elicitation / Continuation)
        if let Some((steps, idx, mut ctx)) = self.pending_execution.take() {
            info!("Resuming pending execution at step {}", idx);
            // Update context with new input
            ctx.input_text = input_text;
            // Resume workflow
            self.execute_workflow(steps, idx, ctx, target_ids, event.source).await;
            return;
        }

        // 1. Perception Layer
        let perception = match self.perception_module.perceive(&event).await {
            Ok(p) => p,
            Err(e) => {
                error!("Perception failed: {}", e);
                return;
            }
        };
        info!("Perception Result: {:?}", perception);

        // 2. Intent & State Layer (The "Soul Question")
        let intent = match self
            .intent_module
            .evaluate(&self.persona, &perception, &input_text)
            .await
        {
            Ok(i) => i,
            Err(e) => {
                error!("Intent evaluation failed: {}", e);
                return;
            }
        };

        if intent == IntentDecision::Ignore {
            info!("IntentDecision: IGNORE. Skipping response.");
            return;
        }

        info!("IntentDecision: ACT. Proceeding to DecisionEngine.");

        // 3. Decision Engine
        let plan_res = self.decision_engine.decide(&self.persona, &event, &*self.mcp_client).await;
        match plan_res {
            Ok(plan) => {
                info!("Plan decided for session {}: {:?}", self.id, plan);

                let mut ctx = crate::utils::Context::new(
                    (*self.persona).clone(),
                    input_text.clone(),
                    Some(self.id.clone()),
                );
                
                // Initialize workflow context in memory
                ctx.memory = serde_json::json!({
                    "workflow": {
                        "plan": plan.clone(),
                        "current_step_index": 0
                    }
                });
                
                self.execute_workflow(plan.steps, 0, ctx, target_ids, event.source).await;
            }
            Err(e) => {
                if e.to_string().contains("NO_TOOLS_AVAILABLE") {
                    info!("No tools available, sending notification.");
                    let output = OutputEvent {
                        target: "default".to_string(),
                        source: event.source.clone(),
                        session_id: Some(self.id.clone()),
                        content: serde_json::json!({
                            "content": "没有可用执行能力"
                        }),
                        style: self.persona.style.clone(),
                    };

                    let handlers_guard = self.output_handlers.read().await;
                    let futures = target_ids
                        .iter()
                        .filter_map(|handler_id| handlers_guard.get(handler_id))
                        .map(|handler| handler.emit(output.clone()))
                        .collect::<Vec<_>>();
                    join_all(futures).await;
                }
                error!("Error deciding plan: {}", e);
            }
        }
    }
    
    async fn execute_workflow(
        &mut self, 
        steps: Vec<StepSpec>, 
        start_idx: usize, 
        mut ctx: Context,
        target_ids: Vec<HandlerId>,
        event_source: String,
    ) {
        for (i, spec) in steps.iter().enumerate().skip(start_idx) {
            // Update current step index in memory
            if let Some(workflow) = ctx.memory.get_mut("workflow") {
                 if let Some(obj) = workflow.as_object_mut() {
                     obj.insert("current_step_index".to_string(), serde_json::json!(i));
                 }
            }

            info!("workflow step start: {:?}", spec);

            let (is_bg, task_name, task_args) = match &spec {
                crate::utils::StepSpec::Tool { name, args, is_background, dependencies: _ } => {
                    (*is_background, name.clone(), Some(args.clone()))
                }
                _ => (false, "background_task".to_string(), None),
            };

            if is_bg {
                info!("Spawning background task for step: {:?}", spec);
                let step = crate::workflow_steps::build_step(
                    &spec,
                    self.workflow_engine.resolver.clone(),
                );
                
                // Clone dependencies for background task
                let mut ctx_clone = ctx.clone();
                let mcp_client = self.mcp_client.clone();
                let output_handlers = self.output_handlers.clone();
                let target_ids_clone = target_ids.clone();
                let session_id = self.id.clone();
                let event_source = event_source.clone();
                let event_source_task = event_source.clone();
                let task_manager = self.task_manager.clone();
                
                let task_id = Uuid::new_v4().to_string();
                let task_id_clone = task_id.clone();
                let original_prompt = match &task_args {
                    Some(args) => {
                        let args_str = args.to_string();
                        if args_str == "null" || args_str == "{}" || args_str == "[]" {
                            ctx.input_text.clone()
                        } else {
                            format!("{} | args={}", ctx.input_text, args_str)
                        }
                    }
                    None => ctx.input_text.clone(),
                };

                let handle = tokio::spawn(async move {
                    let res = step.run(&mut ctx_clone, &*mcp_client).await;
                     match res {
                        Ok(res) => {
                            if let Some(mut o) = res.output {
                                o.source = event_source_task;
                                if o.session_id.is_none() {
                                    o.session_id = Some(session_id);
                                }

                                // Dispatch output
                                let handlers_guard = output_handlers.read().await;
                                let futures = target_ids_clone
                                    .iter()
                                    .filter_map(|handler_id| handlers_guard.get(handler_id))
                                    .map(|handler| handler.emit(o.clone()))
                                    .collect::<Vec<_>>();
                                futures::future::join_all(futures).await;
                            }
                        }
                        Err(e) => {
                            error!("Error executing background workflow step: {}", e);
                        }
                    }
                    // Remove task from manager upon completion
                    task_manager.remove_task(&task_id_clone).await;
                });
                
                self
                    .task_manager
                    .add_task(task_id.clone(), task_name.clone(), original_prompt, handle)
                    .await;
                
                // Notify user that background task started
                let output = OutputEvent {
                    target: "default".into(),
                    source: event_source.clone(),
                    session_id: Some(self.id.clone()),
                    content: serde_json::json!({
                        "type": "text",
                        "text": format!("Started background task '{}' (ID: {})", task_name, task_id)
                    }),
                    style: self.persona.style.clone(),
                };
                let handlers_guard = self.output_handlers.read().await;
                for handler_id in &target_ids {
                    if let Some(handler) = handlers_guard.get(handler_id) {
                        let _ = handler.emit(output.clone()).await;
                    }
                }
                
                continue;
            }

            let step = crate::workflow_steps::build_step(
                &spec,
                self.workflow_engine.resolver.clone(),
            );
            let res = step.run(&mut ctx, &*self.mcp_client).await;

            match res {
                Ok(res) => {
                    // Handle output
                    if let Some(mut o) = res.output {
                        o.source = event_source.clone();
                        if o.session_id.is_none() {
                            o.session_id = Some(self.id.clone());
                        }

                        info!(
                            "workflow step produced output, dispatching to {} handlers",
                            target_ids.len()
                        );

                        // Dispatch output
                        let handlers_guard = self.output_handlers.read().await;
                        let futures = target_ids
                            .iter()
                            .filter_map(|handler_id| handlers_guard.get(handler_id))
                            .map(|handler| handler.emit(o.clone()))
                            .collect::<Vec<_>>();
                        let results = join_all(futures).await;
                        for res in results {
                            if let Err(e) = res {
                                error!("Error emitting workflow output: {}", e);
                            }
                        }
                    }
                    
                    // Handle status
                    match res.status {
                        StepStatus::WaitUser(prompt) => {
                            info!("Workflow step requests user input: {}", prompt);
                            
                            // Emit prompt to user
                            let output = OutputEvent {
                                target: "default".to_string(),
                                source: event_source.clone(),
                                session_id: Some(self.id.clone()),
                                content: serde_json::json!({
                                    "type": "text",
                                    "text": prompt
                                }),
                                style: self.persona.style.clone(),
                            };

                            let handlers_guard = self.output_handlers.read().await;
                            let futures = target_ids
                                .iter()
                                .filter_map(|handler_id| handlers_guard.get(handler_id))
                                .map(|handler| handler.emit(output.clone()))
                                .collect::<Vec<_>>();
                            join_all(futures).await;
                            
                            // Suspend execution
                            self.pending_execution = Some((steps, i, ctx));
                            return;
                        }
                        StepStatus::Stop => {
                            info!("workflow step requests stop");
                            break;
                        }
                        StepStatus::Continue => {
                            // Continue to next step
                        }
                    }
                }
                Err(e) => {
                    error!("Error executing workflow step: {}", e);
                    break;
                }
            }
        }
    }
}

pub struct SessionManager {
    sessions: RwLock<HashMap<String, mpsc::UnboundedSender<SessionMessage>>>,
    factory: Arc<super::McpClientFactory>,

    // Dependencies for spawning sessions
    decision_engine: Arc<Box<dyn DecisionEngine + Send + Sync>>,
    workflow_engine: Arc<WorkflowEngine>,
    perception_module: Arc<Box<dyn PerceptionModule + Send + Sync>>,
    intent_module: Arc<Box<dyn IntentModule + Send + Sync>>,
    persona: Arc<Persona>,
    output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>>,
    router: Arc<StdRwLock<EventRouter>>,
}

impl SessionManager {
    pub fn new(
        factory: Arc<super::McpClientFactory>,
        decision_engine: Arc<Box<dyn DecisionEngine + Send + Sync>>,
        workflow_engine: Arc<WorkflowEngine>,
        perception_module: Arc<Box<dyn PerceptionModule + Send + Sync>>,
        intent_module: Arc<Box<dyn IntentModule + Send + Sync>>,
        persona: Arc<Persona>,
        output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>>,
        router: Arc<StdRwLock<EventRouter>>,
    ) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            factory,
            decision_engine,
            workflow_engine,
            perception_module,
            intent_module,
            persona,
            output_handlers,
            router,
        }
    }

    pub async fn dispatch(&self, event: InputEvent) {
        let session_id = event
            .session_id
            .clone()
            .unwrap_or_else(|| event.source.clone());

        // Fast path: check if session exists with read lock
        {
            let guard = self.sessions.read().await;
            if let Some(sender) = guard.get(&session_id) {
                if sender.send(SessionMessage::Input(event.clone())).is_ok() {
                    return;
                }
                // If send failed, channel is closed, we need to recreate session
            }
        }

        // Slow path: create session with write lock
        let mut guard = self.sessions.write().await;
        // Check again in case someone else created it
        if let Some(sender) = guard.get(&session_id) {
            if sender.send(SessionMessage::Input(event.clone())).is_ok() {
                return;
            }
        }

        // Create new session
        info!("Creating new session actor for {}", session_id);
        match (self.factory)(session_id.clone()).await {
            Ok(mcp_client) => {
                let (tx, rx) = mpsc::unbounded_channel();

                let task_manager = Arc::new(TaskManager::new());
                let aware_client: Arc<dyn MCPClient + Send + Sync> = Arc::new(TaskAwareMcpClient::new(mcp_client, task_manager.clone()));
                let decision_engine = self.decision_engine.clone();
                let workflow_engine = self.workflow_engine.clone();

                let session = RobotSession {
                    id: session_id.clone(),
                    inbox: rx,
                    mcp_client: aware_client,
                    decision_engine,
                    workflow_engine,
                    perception_module: self.perception_module.clone(),
                    intent_module: self.intent_module.clone(),
                    persona: self.persona.clone(),
                    output_handlers: self.output_handlers.clone(),
                    router: self.router.clone(),
                    task_manager,
                    pending_execution: None,
                };

                // Spawn session actor
                let actor: Box<dyn SessionActor> = if event.source == "web" {
                    Box::new(WebSession { inner: session })
                } else {
                    Box::new(session)
                };
                tokio::spawn(actor.run());

                // Store sender and dispatch
                guard.insert(session_id.clone(), tx.clone());
                if let Err(e) = tx.send(SessionMessage::Input(event)) {
                    error!(
                        "Failed to dispatch event to new session {}: {}",
                        session_id, e
                    );
                }
            }
            Err(e) => {
                error!(
                    "Failed to create MCP client for session {}: {}",
                    session_id, e
                );
            }
        }
    }
}
