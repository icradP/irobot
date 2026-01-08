#[macro_use]
extern crate robot_core;

use robot_core::core::{
    decision_engine::LLMDecisionEngine, persona::Persona, stdin_manager::StdinManager,
    workflow_engine::WorkflowEngine, RobotCore,
};
use robot_core::llm::lmstudio::LMStudioClient;
use robot_core::mcp::rmcp_client::RmcpStdIoClient;
use robot_core::tentacles::console::{ConsoleHandler, ConsoleInput, ConsoleOutput};
use robot_core::tentacles::web_console::{WebHandler, WebInput, WebOutput};
use robot_core::workflow_steps::LlmParameterResolver;
use std::sync::Arc;
use tracing_subscriber;
use url::Url;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "info,rmcp=info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let persona = Persona::default();
    let base =
        std::env::var("LMSTUDIO_URL").unwrap_or_else(|_| "http://localhost:1234".to_string());
    let url = Url::parse(&base)?;
    let api_key = std::env::var("LMSTUDIO_API_KEY").ok();
    let model = std::env::var("LMSTUDIO_MODEL").unwrap_or_else(|_| "default".to_string());
    let llm_for_decision = LMStudioClient::new(url.clone(), api_key.clone());

    // Create shared StdinManager
    let stdin_manager = StdinManager::new();

    // Create MCP client with standard rmcp implementation
    let mcp_client = Arc::new(RmcpStdIoClient::new(
        stdin_manager.clone(),
        Arc::new(llm_for_decision.clone()),
        model.clone()
    ).await?);
    let decision = Box::new(LLMDecisionEngine::new(
        Box::new(llm_for_decision),
        model,
        mcp_client.clone(),
    ));
    let param_resolver = Arc::new(LlmParameterResolver {
        llm: Arc::new(LMStudioClient::new(url.clone(), api_key.clone())),
        model: std::env::var("LMSTUDIO_MODEL").unwrap_or_else(|_| "default".to_string()),
    });
    let workflow = WorkflowEngine::new_with_resolver(param_resolver);
    // Removed second instantiation
    
    let mut core = RobotCore::new(persona, decision, workflow, mcp_client);

    register_handlers!(core => {
        ConsoleHandler: (
            ConsoleInput::new(&stdin_manager),
            ConsoleOutput
        ) -> [ConsoleHandler],

        WebHandler: (
            WebInput::new(8080, stdin_manager.clone()).await?,
            WebOutput::new(8081).await?
        ) -> [WebHandler],
    });

    loop {
        core.run_once().await?;
    }
}
