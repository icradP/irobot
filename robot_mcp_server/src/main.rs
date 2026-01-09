use anyhow::Result;
use rmcp::{
    ErrorData, ServerHandler, ServiceExt, elicit_safe,
    model::*,
    service::{RequestContext, RoleServer},
};
use std::sync::{Arc, Mutex};
use tracing_subscriber::{self, EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};
mod tools;
use crate::tools::AppState;
mod external;

elicit_safe!();

#[derive(Clone)]
pub struct RobotService {
    state: Arc<Mutex<tools::AppState>>,
    externals: Arc<external::ExternalManager>,
}

impl RobotService {
    pub async fn new() -> anyhow::Result<Self> {
        let externals = external::ExternalManager::new_from_config().await?;
        Ok(Self {
            state: Arc::new(Mutex::new(AppState::default())),
            externals: Arc::new(externals),
        })
    }
}

impl ServerHandler for RobotService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some("Robot MCP Server with Memory and Profile capabilities".into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut list = tools::all_tools();
        match self.externals.list_tools().await {
            Ok(mut ext) => {
                list.append(&mut ext);
            }
            Err(e) => {
                tracing::warn!("List external tools failed: {}", e);
            }
        }
        Ok(ListToolsResult {
            tools: list,
            meta: None,
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        if external::parse_external_name(&request.name).is_some() {
            match self
                .externals
                .call_external(
                    &request.name,
                    request.arguments.map(|v| v.into()),
                    context.clone(),
                )
                .await
            {
                Ok(res) => Ok(res),
                Err(e) => Err(ErrorData::internal_error(
                    format!("外部调用失败: {}", e),
                    None,
                )),
            }
        } else {
            tools::dispatch(
                &request.name,
                request.arguments.map(|v| v.into()),
                context,
                self.state.clone(),
            )
            .await
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(false),
        )
        .init();

    // TCP server address from environment or default
    let bind_addr =
        std::env::var("ROBOT_MCP_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:9001".to_string());

    let listener = tokio::net::TcpListener::bind(&bind_addr).await?;
    tracing::info!("Robot MCP Server listening on: {}", bind_addr);

    loop {
        let (stream, peer_addr) = listener.accept().await?;
        tracing::info!("Accepted connection from: {}", peer_addr);

        tokio::spawn(async move {
            match RobotService::new().await {
                Ok(service) => match service.serve(stream).await {
                    Ok(server) => {
                        tracing::info!("Service initialized for {}", peer_addr);
                        if let Err(e) = server.waiting().await {
                            tracing::error!("Service error for {}: {:?}", peer_addr, e);
                        }
                        tracing::info!("Service closed for {}", peer_addr);
                    }
                    Err(e) => {
                        tracing::error!("Service run error for {}: {:?}", peer_addr, e);
                    }
                },
                Err(e) => {
                    tracing::error!("Service initialization error for {}: {:?}", peer_addr, e);
                }
            }
        });
    }
}
