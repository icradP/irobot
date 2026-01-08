#[macro_use]
extern crate robot_core;

use robot_core::core::{decision_engine::LLMDecisionEngine, persona::Persona, workflow_engine::WorkflowEngine, RobotCore};
use robot_core::llm::lmstudio::LMStudioClient;
use robot_core::mcp::rmcp_client::RmcpStdIoClient;
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

    // Create MCP client for decision engine (system session)
    let mcp_client = Arc::new(RmcpStdIoClient::new(
        Arc::new(llm_for_decision.clone()),
        model.clone(),
        "system".to_string()
    ).await?);
    let decision = Box::new(LLMDecisionEngine::new(
        Box::new(llm_for_decision),
        model.clone(),
        mcp_client.clone(),
    ));
    let param_resolver = Arc::new(LlmParameterResolver {
        llm: Arc::new(LMStudioClient::new(url.clone(), api_key.clone())),
        model: std::env::var("LMSTUDIO_MODEL").unwrap_or_else(|_| "default".to_string()),
    });
    let workflow = WorkflowEngine::new_with_resolver(param_resolver);
    
    // Create factory for per-session clients
    let factory_url = url.clone();
    let factory_api_key = api_key.clone();
    let factory_model = model.clone();
    
    let mcp_client_factory: robot_core::core::McpClientFactory = Box::new(move |session_id: String| {
        let url = factory_url.clone();
        let api_key = factory_api_key.clone();
        let model = factory_model.clone();
        
        Box::pin(async move {
            let llm = LMStudioClient::new(url, api_key);
            let client = RmcpStdIoClient::new(
                Arc::new(llm),
                model,
                session_id,
            ).await?;
            Ok(Arc::new(client) as Arc<dyn robot_core::mcp::client::MCPClient + Send + Sync>)
        })
    });

    let mut core = RobotCore::new(persona, decision, workflow, mcp_client_factory);

    register_handlers!(core => {
        WebHandler: (
            WebInput::new(8080).await?,
            WebOutput::new(8081).await?
        ) -> [WebHandler],
    });

    loop {
        core.run_once().await?;
    }
}
