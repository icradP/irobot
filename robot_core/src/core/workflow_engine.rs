use crate::core::output_handler::OutputHandler;
use crate::core::persona::Persona;
use crate::mcp::client::MCPClient;
use crate::utils::WorkflowPlan;
use crate::workflow_steps::{build_step, StepResult, ParameterResolver, NoopResolver};
use tracing::info;
use std::sync::Arc;

pub struct WorkflowEngine {
    pub resolver: Arc<dyn ParameterResolver + Send + Sync>,
}

impl WorkflowEngine {
    pub fn new() -> Self {
        Self { resolver: Arc::new(NoopResolver) }
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
                
                info!("workflow step produced output, dispatching to {} handlers", outputs.len());
                for h in outputs {
                    h.emit(o.clone()).await?;
                }
            }
            if !res.next {
                info!("workflow step requests stop");
                break;
            }
            info!("workflow step done: {:?}", spec);
        }
        info!("workflow execute complete");
        Ok(())
    }
}
