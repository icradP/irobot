pub mod decision_engine;
pub mod input_handler;
pub mod output_handler;
pub mod persona;
pub mod router;
pub mod workflow_engine;

use crate::core::decision_engine::DecisionEngine;
use crate::core::input_handler::InputHandler;
use crate::core::output_handler::OutputHandler;
use crate::core::persona::Persona;
use crate::core::router::{EventRouter, HandlerId};
use crate::core::workflow_engine::WorkflowEngine;
use crate::mcp::client::MCPClient;
use crate::utils::InputEvent;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::{info, error};
use futures::future::{join_all, BoxFuture};

use std::sync::Arc;
use tokio::sync::RwLock;

pub type McpClientFactory = Box<dyn Fn(String) -> BoxFuture<'static, anyhow::Result<Arc<dyn MCPClient + Send + Sync>>> + Send + Sync>;

pub struct RobotCore {
    pub persona: Arc<Persona>,
    pub decision_engine: Arc<Box<dyn DecisionEngine + Send + Sync>>,
    pub workflow_engine: Arc<WorkflowEngine>,
    pub output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>>,
    pub mcp_clients: Arc<RwLock<HashMap<String, Arc<dyn MCPClient + Send + Sync>>>>,
    pub mcp_client_factory: Arc<McpClientFactory>,
    pub input_receiver: mpsc::UnboundedReceiver<InputEvent>,
    pub input_sender: mpsc::UnboundedSender<InputEvent>,
    pub router: Arc<EventRouter>,
}

impl RobotCore {
    pub fn new(
        persona: Persona,
        decision_engine: Box<dyn DecisionEngine + Send + Sync>,
        workflow_engine: WorkflowEngine,
        mcp_client_factory: McpClientFactory,
    ) -> Self {
        let (input_sender, input_receiver) = mpsc::unbounded_channel();
        let output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>> = Arc::new(RwLock::new(HashMap::new()));
        let mcp_clients = Arc::new(RwLock::new(HashMap::new()));
        
        // Spawn background task for system output broadcasting
        let handlers_clone = output_handlers.clone();
        tokio::spawn(async move {
            let mut output_bus_receiver = crate::utils::output_bus().subscribe();
            while let Ok(event) = output_bus_receiver.recv().await {
                info!("Broadcasting system output from {}", event.source);
                let handlers_guard = handlers_clone.read().await;
                
                // Collect futures to await them
                let futures = handlers_guard.values()
                    .map(|handler| handler.emit(event.clone()))
                    .collect::<Vec<_>>();
                
                let results = join_all(futures).await;
                for res in results {
                    if let Err(e) = res {
                        info!("Error emitting system output: {}", e);
                    }
                }
            }
        });

        Self {
            persona: Arc::new(persona),
            decision_engine: Arc::new(decision_engine),
            workflow_engine: Arc::new(workflow_engine),
            output_handlers,
            mcp_clients,
            mcp_client_factory: Arc::new(mcp_client_factory),
            input_receiver,
            input_sender,
            router: Arc::new(EventRouter::new()),
        }
    }

    pub fn add_input_handler(&self, handler: Box<dyn InputHandler + Send + Sync>) {
        let sender = self.input_sender.clone();
        tokio::spawn(async move {
            loop {
                match handler.poll().await {
                    Ok(Some(event)) => {
                        info!("Received event from {}", event.source);
                        if sender.send(event).is_err() {
                            info!("Input handler: main channel closed, stopping");
                            break;
                        }
                    }
                    Ok(None) => {
                        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    }
                    Err(e) => {
                        info!("Input handler error: {}, retrying...", e);
                        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                    }
                }
            }
        });
    }

    pub async fn add_output_handler(
        &mut self,
        id: HandlerId,
        handler: Box<dyn OutputHandler + Send + Sync>,
    ) {
        self.output_handlers.write().await.insert(id, handler);
    }

    pub fn route(&mut self) -> &mut EventRouter {
        Arc::get_mut(&mut self.router).expect("Router should not be shared when configuring")
    }
    pub async fn run_once(&mut self) -> anyhow::Result<()> {
        tokio::select! {
            res = self.input_receiver.recv() => {
                if let Some(event) = res {
                    // Check if event was already consumed by an MCP elicitation process
                    if crate::utils::check_and_remove_consumed_event(&event.id) {
                        info!("Skipping event {} as it was consumed by MCP elicitation", event.id);
                        return Ok(());
                    }

                    info!("Processing event from {}", event.source);
                    
                    // Clone Arcs for the task
                    let decision_engine = self.decision_engine.clone();
                    let persona = self.persona.clone();
                    let workflow_engine = self.workflow_engine.clone();
                    let mcp_clients = self.mcp_clients.clone();
                    let mcp_client_factory = self.mcp_client_factory.clone();
                    let output_handlers = self.output_handlers.clone();
                    let router = self.router.clone();
                    
                    tokio::spawn(async move {
                        let session_id = event.session_id.clone().unwrap_or_else(|| event.source.clone());

                        // Get or create client
                        let mcp_client = {
                            // Try read first
                            let client_opt = {
                                let guard = mcp_clients.read().await;
                                guard.get(&session_id).cloned()
                            };
                            
                            if let Some(client) = client_opt {
                                client
                            } else {
                                info!("Creating new MCP client for session {}", session_id);
                                match mcp_client_factory(session_id.clone()).await {
                                    Ok(client) => {
                                        let mut guard = mcp_clients.write().await;
                                        // Double check
                                        if let Some(existing) = guard.get(&session_id) {
                                            existing.clone()
                                        } else {
                                            guard.insert(session_id.clone(), client.clone());
                                            client
                                        }
                                    },
                                    Err(e) => {
                                        error!("Failed to create MCP client for session {}: {}", session_id, e);
                                        return; 
                                    }
                                }
                            }
                        };

                        let plan_res = decision_engine.decide(&persona, &event).await;
                        match plan_res {
                            Ok(plan) => {
                                info!("Plan decided: {:?}", plan);
            
                                let input_text = if let Some(line) =
                                    event.payload.get("line").and_then(|v: &serde_json::Value| v.as_str())
                                {
                                    line.to_string()
                                } else if let Some(content) = event.payload.get("content").and_then(|v: &serde_json::Value| v.as_str())
                                {
                                    content.to_string()
                                } else {
                                    String::new()
                                };
            
                                // Get target output handler IDs based on routing
                                let target_ids = if router.has_routes() {
                                    let route_ids = router.get_outputs_for_event(&event);
                                    if !route_ids.is_empty() {
                                        route_ids
                                    } else {
                                        output_handlers.read().await.keys().cloned().collect()
                                    }
                                } else {
                                    output_handlers.read().await.keys().cloned().collect()
                                };
            
                                info!(
                                    "Routing event from '{}' to {} handlers",
                                    event.source,
                                    target_ids.len()
                                );
            
                                // Execute workflow and emit to routed handlers
                                let mut ctx = crate::utils::Context::new(
                                    (*persona).clone(),
                                    input_text,
                                    Some(event.session_id.clone().unwrap_or_else(|| event.source.clone())),
                                );
                                for spec in plan.steps {
                                    info!("workflow step start: {:?}", spec);
                                    let step = crate::workflow_steps::build_step(
                                        &spec,
                                        workflow_engine.resolver.clone(),
                                    );
                                    let res = step.run(&mut ctx, &*mcp_client).await;
                                    
                                    match res {
                                        Ok(res) => {
                                            if let Some(mut o) = res.output {
                                                o.source = event.source.clone();
                                                if o.session_id.is_none() {
                                                    o.session_id = Some(event.session_id.clone().unwrap_or_else(|| event.source.clone()));
                                                }
                            
                                                info!(
                                                    "workflow step produced output, dispatching to {} handlers",
                                                    target_ids.len()
                                                );
                                                
                                                // Need to acquire read lock to get the handlers
                                                let handlers_guard = output_handlers.read().await;
                                                let futures = target_ids
                                                    .iter()
                                                    .filter_map(|handler_id| handlers_guard.get(handler_id))
                                                    .map(|handler| handler.emit(o.clone()))
                                                    .collect::<Vec<_>>();
                                                let results = join_all(futures).await;
                                                for res in results {
                                                    if let Err(e) = res {
                                                        info!("Error emitting workflow output: {}", e);
                                                    }
                                                }
                                            }
                                            if !res.next {
                                                info!("workflow step requests stop");
                                                break;
                                            }
                                        }
                                        Err(e) => {
                                            info!("Error executing workflow step: {}", e);
                                            break;
                                        }
                                    }
                                }
                            },
                            Err(e) => {
                                info!("Error deciding plan: {}", e);
                            }
                        }
                    });
                } else {
                    info!("Input channel closed");
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_millis(100)) => {
                // Idle
            }
        }

        Ok(())
    }
}
