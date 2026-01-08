pub mod decision_engine;
pub mod input_handler;
pub mod output_handler;
pub mod persona;
pub mod router;
pub mod stdin_manager;
pub mod workflow_engine;

pub use stdin_manager::StdinManager;

use crate::core::decision_engine::DecisionEngine;
use crate::core::input_handler::InputHandler;
use crate::core::output_handler::OutputHandler;
use crate::core::persona::Persona;
use crate::core::router::{EventRouter, HandlerId};
use crate::core::workflow_engine::WorkflowEngine;
use crate::mcp::client::MCPClient;
use crate::utils::{InputEvent, WorkflowPlan};
use std::collections::HashMap;
use tokio::sync::mpsc;
use tracing::info;

use std::sync::Arc;

pub struct RobotCore {
    pub persona: Persona,
    pub decision_engine: Box<dyn DecisionEngine + Send + Sync>,
    pub workflow_engine: WorkflowEngine,
    pub output_handlers: HashMap<HandlerId, Box<dyn OutputHandler + Send + Sync>>,
    pub mcp_client: Arc<dyn MCPClient + Send + Sync>,
    pub input_receiver: mpsc::UnboundedReceiver<InputEvent>,
    pub input_sender: mpsc::UnboundedSender<InputEvent>,
    pub router: EventRouter,
}

impl RobotCore {
    pub fn new(
        persona: Persona,
        decision_engine: Box<dyn DecisionEngine + Send + Sync>,
        workflow_engine: WorkflowEngine,
        mcp_client: Arc<dyn MCPClient + Send + Sync>,
    ) -> Self {
        let (input_sender, input_receiver) = mpsc::unbounded_channel();

        Self {
            persona,
            decision_engine,
            workflow_engine,
            output_handlers: HashMap::new(),
            mcp_client,
            input_receiver,
            input_sender,
            router: EventRouter::new(),
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

    pub fn add_output_handler(
        &mut self,
        id: HandlerId,
        handler: Box<dyn OutputHandler + Send + Sync>,
    ) {
        self.output_handlers.insert(id, handler);
    }

    pub fn route(&mut self) -> &mut EventRouter {
        &mut self.router
    }

    /// Get output handlers for an event based on routing rules
    /// Returns all handlers if no routing is configured
    fn get_target_handlers(&self, event: &InputEvent) -> Vec<HandlerId> {
        if self.router.has_routes() {
            let route_ids = self.router.get_outputs_for_event(event);
            if !route_ids.is_empty() {
                return route_ids;
            }
        }
        // Default: return all handler IDs
        self.output_handlers.keys().cloned().collect()
    }

    pub async fn run_once(&mut self) -> anyhow::Result<()> {
        match tokio::time::timeout(
            tokio::time::Duration::from_millis(100),
            self.input_receiver.recv(),
        )
        .await
        {
            Ok(Some(event)) => {
                info!("Processing event from {}", event.source);
                let plan: WorkflowPlan = self.decision_engine.decide(&self.persona, &event).await?;
                info!("Plan decided: {:?}", plan);

                let input_text = if let Some(line) =
                    event.payload.get("line").and_then(|v| v.as_str())
                {
                    line.to_string()
                } else if let Some(content) = event.payload.get("content").and_then(|v| v.as_str())
                {
                    content.to_string()
                } else {
                    String::new()
                };

                // Get target output handler IDs based on routing
                let target_ids = self.get_target_handlers(&event);

                info!(
                    "Routing event from '{}' to {} handlers",
                    event.source,
                    target_ids.len()
                );

                // Execute workflow and emit to routed handlers
                let mut ctx = crate::utils::Context::new(self.persona.clone(), input_text);
                for spec in plan.steps {
                    info!("workflow step start: {:?}", spec);
                    let step = crate::workflow_steps::build_step(
                        &spec,
                        self.workflow_engine.resolver.clone(),
                    );
                    let res: crate::workflow_steps::StepResult =
                        step.run(&mut ctx, &*self.mcp_client).await?;
                    if let Some(mut o) = res.output {
                        o.source = event.source.clone();

                        info!(
                            "workflow step produced output, dispatching to {} handlers",
                            target_ids.len()
                        );
                        for handler_id in &target_ids {
                            if let Some(handler) = self.output_handlers.get(handler_id) {
                                handler.emit(o.clone()).await?;
                            }
                        }
                    }
                    if !res.next {
                        info!("workflow step requests stop");
                        break;
                    }
                }
            }
            Ok(None) => {
                info!("Input channel closed");
            }
            Err(_) => {
                // Timeout - continue silently
            }
        }

        Ok(())
    }
}
