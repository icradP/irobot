use crate::core::output_handler::OutputHandler;
use crate::core::persona::Persona;
use crate::mcp::client::MCPClient;
use crate::utils::WorkflowPlan;
use crate::workflow_steps::{NoopResolver, ParameterResolver, StepResult, StepStatus, build_step};
use std::sync::Arc;
use tracing::info;

pub struct WorkflowEngine {
    pub resolver: Arc<dyn ParameterResolver + Send + Sync>,
}

impl WorkflowEngine {
    pub fn new() -> Self {
        Self {
            resolver: Arc::new(NoopResolver),
        }
    }
    pub fn new_with_resolver(resolver: Arc<dyn ParameterResolver + Send + Sync>) -> Self {
        Self { resolver }
    }

    pub async fn execute_simple(
        &self,
        plan: WorkflowPlan,
        persona: &Persona,
        mcp: &dyn MCPClient,
        outputs: &[Box<dyn OutputHandler + Send + Sync>],
        input_text: String,
        input_source: String,
    ) -> anyhow::Result<()> {
        let mut ctx = crate::utils::Context::new(persona.clone(), input_text, None);
        for spec in plan.steps {
            info!("workflow step start: {:?}", spec);
            let step = build_step(&spec, self.resolver.clone());
            let res: StepResult = step.run(&mut ctx, mcp).await?;
            if let Some(mut o) = res.output {
                o.source = input_source.clone();

                info!(
                    "workflow step produced output, dispatching to {} handlers",
                    outputs.len()
                );
                for h in outputs {
                    h.emit(o.clone()).await?;
                }
            }
            match res.status {
                StepStatus::Stop => {
                    info!("workflow step requests stop");
                    break;
                }
                StepStatus::WaitUser(prompt) => {
                    info!("workflow step requests user input: {}", prompt);
                    // Emit prompt
                    let output = crate::utils::OutputEvent {
                        target: "default".into(),
                        source: input_source.clone(),
                        session_id: None,
                        content: serde_json::json!({"type": "text", "text": prompt}),
                        style: persona.style.clone(),
                    };
                    for h in outputs {
                         let _ = h.emit(output.clone()).await;
                    }
                    break;
                }
                StepStatus::Continue => {}
            }
            info!("workflow step done: {:?}", spec);
        }
        info!("workflow execute complete");
        Ok(())
    }
}
