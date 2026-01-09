use crate::core::decision_engine::DecisionEngine;
use crate::core::intent::{IntentDecision, IntentModule};
use crate::core::output_handler::OutputHandler;
use crate::core::perception::PerceptionModule;
use crate::core::persona::Persona;
use crate::core::router::{EventRouter, HandlerId};
use crate::core::sessions::web_session::WebSession;
use crate::core::workflow_engine::WorkflowEngine;
use crate::mcp::client::MCPClient;
use crate::utils::InputEvent;
use async_trait::async_trait;
use futures::future::join_all;
use std::collections::HashMap;
use std::sync::{Arc, RwLock as StdRwLock};
use tokio::sync::{mpsc, RwLock};
use tracing::{error, info};

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
}

#[async_trait]
impl SessionActor for RobotSession {
    async fn run(self: Box<Self>) {
        (*self).run_inner().await;
    }
}

impl RobotSession {
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

        // 1. Perception Layer
        let perception = match self.perception_module.perceive(&event).await {
            Ok(p) => p,
            Err(e) => {
                error!("Perception failed: {}", e);
                return;
            }
        };
        info!("Perception Result: {:?}", perception);

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
        let plan_res = self.decision_engine.decide(&self.persona, &event).await;
        match plan_res {
            Ok(plan) => {
                info!("Plan decided for session {}: {:?}", self.id, plan);

                // Routing logic
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

                let mut ctx = crate::utils::Context::new(
                    (*self.persona).clone(),
                    input_text,
                    Some(self.id.clone()),
                );

                for spec in plan.steps {
                    info!("workflow step start: {:?}", spec);
                    let step = crate::workflow_steps::build_step(
                        &spec,
                        self.workflow_engine.resolver.clone(),
                    );
                    let res = step.run(&mut ctx, &*self.mcp_client).await;

                    match res {
                        Ok(res) => {
                            if let Some(mut o) = res.output {
                                o.source = event.source.clone();
                                if o.session_id.is_none() {
                                    o.session_id = Some(self.id.clone());
                                }

                                info!(
                                    "workflow step produced output, dispatching to {} handlers",
                                    target_ids.len()
                                );

                                // Dispatch output
                                // Note: We acquire read lock briefly to get handlers, then emit
                                // Ideally we should clone handlers if possible to avoid holding lock during emit
                                // But OutputHandler is a Trait Object in a Box, so cloning is hard unless we use Arc<Box<dyn...>>
                                // For now, we follow existing pattern but we should optimize locking strategy later
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
                            if !res.next {
                                info!("workflow step requests stop");
                                break;
                            }
                        }
                        Err(e) => {
                            error!("Error executing workflow step: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                error!("Error deciding plan: {}", e);
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
                let session = RobotSession {
                    id: session_id.clone(),
                    inbox: rx,
                    mcp_client,
                    decision_engine: self.decision_engine.clone(),
                    workflow_engine: self.workflow_engine.clone(),
                    perception_module: self.perception_module.clone(),
                    intent_module: self.intent_module.clone(),
                    persona: self.persona.clone(),
                    output_handlers: self.output_handlers.clone(),
                    router: self.router.clone(),
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
