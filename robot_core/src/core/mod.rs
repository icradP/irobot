pub mod decision_engine;
pub mod input_handler;
pub mod intent;
pub mod output_handler;
pub mod perception;
pub mod persona;
pub mod router;
pub mod session;
pub mod sessions;
pub mod workflow_engine;
pub mod tasks;

use crate::core::decision_engine::DecisionEngine;
use crate::core::input_handler::InputHandler;
use crate::core::intent::IntentModule;
use crate::core::output_handler::OutputHandler;
use crate::core::perception::PerceptionModule;
use crate::core::persona::Persona;
use crate::core::router::{EventRouter, HandlerId};
use crate::core::session::SessionManager;
use crate::core::workflow_engine::WorkflowEngine;
use crate::mcp::client::MCPClient;
use crate::utils::InputEvent;
use futures::future::{join_all, BoxFuture};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::info;

use std::sync::{Arc, RwLock as StdRwLock};
use tokio::sync::RwLock;

pub type McpClientFactory = Box<
    dyn Fn(String) -> BoxFuture<'static, anyhow::Result<Arc<dyn MCPClient + Send + Sync>>>
        + Send
        + Sync,
>;

pub struct RobotCore {
    pub persona: Arc<Persona>,
    pub decision_engine: Arc<Box<dyn DecisionEngine + Send + Sync>>,
    pub workflow_engine: Arc<WorkflowEngine>,
    pub perception_module: Arc<Box<dyn PerceptionModule + Send + Sync>>,
    pub intent_module: Arc<Box<dyn IntentModule + Send + Sync>>,
    pub output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>>,
    // mcp_clients is replaced by session_manager
    pub session_manager: Arc<SessionManager>,
    pub mcp_client_factory: Arc<McpClientFactory>,
    pub input_receiver: mpsc::UnboundedReceiver<InputEvent>,
    pub input_sender: mpsc::UnboundedSender<InputEvent>,
    pub router: Arc<StdRwLock<EventRouter>>,
}

impl RobotCore {
    pub fn new(
        persona: Persona,
        decision_engine: Box<dyn DecisionEngine + Send + Sync>,
        workflow_engine: WorkflowEngine,
        perception_module: Box<dyn PerceptionModule + Send + Sync>,
        intent_module: Box<dyn IntentModule + Send + Sync>,
        mcp_client_factory: McpClientFactory,
    ) -> Self {
        let (input_sender, input_receiver) = mpsc::unbounded_channel();
        let output_handlers: Arc<RwLock<HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>>> =
            Arc::new(RwLock::new(HashMap::new()));

        let persona_arc = Arc::new(persona);
        let decision_engine_arc = Arc::new(decision_engine);
        let workflow_engine_arc = Arc::new(workflow_engine);
        let perception_module_arc = Arc::new(perception_module);
        let intent_module_arc = Arc::new(intent_module);
        let mcp_client_factory_arc = Arc::new(mcp_client_factory);
        let router_arc = Arc::new(StdRwLock::new(EventRouter::new()));

        let session_manager = Arc::new(SessionManager::new(
            mcp_client_factory_arc.clone(),
            decision_engine_arc.clone(),
            workflow_engine_arc.clone(),
            perception_module_arc.clone(),
            intent_module_arc.clone(),
            persona_arc.clone(),
            output_handlers.clone(),
            router_arc.clone(),
        ));

        // Spawn background task for system output broadcasting
        let handlers_clone = output_handlers.clone();
        tokio::spawn(async move {
            let mut output_bus_receiver = crate::utils::output_bus().subscribe();
            while let Ok(event) = output_bus_receiver.recv().await {
                info!("Broadcasting system output from {}", event.source);
                let handlers_guard = handlers_clone.read().await;

                // Collect futures to await them
                let futures = handlers_guard
                    .values()
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
            persona: persona_arc,
            decision_engine: decision_engine_arc,
            workflow_engine: workflow_engine_arc,
            perception_module: perception_module_arc,
            intent_module: intent_module_arc,
            output_handlers,
            session_manager,
            mcp_client_factory: mcp_client_factory_arc,
            input_receiver,
            input_sender,
            router: router_arc,
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

    pub fn route(&self) -> std::sync::RwLockWriteGuard<'_, EventRouter> {
        self.router.write().expect("Failed to lock router")
    }

    pub async fn run_once(&mut self) -> anyhow::Result<()> {
        tokio::select! {
            res = self.input_receiver.recv() => {
                if let Some(event) = res {
                    info!("Dispatching event from {} to session manager", event.source);
                    self.session_manager.dispatch(event).await;
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
