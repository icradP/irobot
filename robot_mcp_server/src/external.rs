use anyhow::Result;
use rmcp::{
    model::{object, CallToolRequestParam, CallToolResult, Tool, CreateElicitationRequestParam, CreateElicitationResult, ElicitationAction},
    service::{RequestContext, RoleClient, RoleServer, RunningService},
    ServiceExt,
};
use rmcp::model::{ClientCapabilities, ClientInfo, Implementation};
use std::sync::{Arc};
use std::borrow::Cow;
use tokio::sync::Mutex;
use futures::future::BoxFuture;
use schemars::JsonSchema;
use serde::{Serialize, Deserialize};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransport;
use std::fs;
use std::path::Path;

type BridgeFn = Arc<dyn Fn(String, serde_json::Value) -> BoxFuture<'static, Result<Option<serde_json::Value>>> + Send + Sync>;

#[derive(Debug, Serialize, Deserialize)]
struct ExternalConfig {
    externals: Vec<String>,
}

pub struct BridgeShared {
    pub hook: Arc<Mutex<Option<BridgeFn>>>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BridgeRaw {
    pub raw: String,
}

pub struct BridgeClientHandler {
    info: ClientInfo,
    shared: Arc<BridgeShared>,
}

impl BridgeClientHandler {
    pub fn new(shared: Arc<BridgeShared>) -> Self {
        let info = ClientInfo {
            protocol_version: Default::default(),
            capabilities: ClientCapabilities::builder().enable_elicitation().build(),
            client_info: Implementation {
                name: "robot-mcp-external-client".to_string(),
                title: None,
                version: "0.1.0".to_string(),
                website_url: None,
                icons: None,
            },
        };
        Self { info, shared }
    }
}

impl rmcp::handler::client::ClientHandler for BridgeClientHandler {
    fn get_info(&self) -> ClientInfo {
        self.info.clone()
    }

    fn create_elicitation(
        &self,
        request: CreateElicitationRequestParam,
        _context: RequestContext<RoleClient>,
    ) -> impl std::future::Future<Output = Result<CreateElicitationResult, rmcp::ErrorData>> + Send + '_ {
        async move {
            let schema_str = serde_json::to_string_pretty(&request.requested_schema).unwrap_or_default();
            let msg = format!(
                "外部服务器请求参数引导：{}\nSchema:\n{}\n请提供 JSON 或自然语言描述。",
                request.message, schema_str
            );
            let hook_opt = { self.shared.hook.lock().await.clone() };
            if let Some(hook) = hook_opt {
                match hook(msg, serde_json::to_value(&request.requested_schema).unwrap_or(serde_json::Value::Null)).await {
                    Ok(Some(mut v)) => {
                        // If user provided a JSON string, try to parse into JSON
                        if let serde_json::Value::String(s) = &v {
                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                                v = parsed;
                            }
                        }
                        Ok(CreateElicitationResult {
                            action: ElicitationAction::Accept,
                            content: Some(v),
                        })
                    }
                    Ok(None) => Ok(CreateElicitationResult { action: ElicitationAction::Accept, content: None }),
                    Err(e) => Err(rmcp::ErrorData::internal_error(format!("桥接引导失败: {}", e), None)),
                }
            } else {
                Ok(CreateElicitationResult { action: ElicitationAction::Accept, content: None })
            }
        }
    }
}

pub struct ExternalManager {
    clients: Vec<(String, RunningService<RoleClient, BridgeClientHandler>, Arc<BridgeShared>)>,
}

impl ExternalManager {
    pub async fn new_from_config() -> Result<Self> {
        let mut clients = Vec::new();
        let default_path = "config/external.toml";
        let path = std::env::var("ROBOT_MCP_EXTERNALS_CONFIG").unwrap_or_else(|_| default_path.to_string());
        let p = Path::new(&path);
        if !p.exists() {
            tracing::warn!("外部配置文件不存在: {}，将不加载外部服务", path);
            return Ok(Self { clients });
        }
        let content = fs::read_to_string(p)?;
        let cfg: ExternalConfig = toml::from_str(&content)?;
        for addr in cfg.externals.into_iter() {
            let addr = addr.trim();
            if addr.is_empty() {
                continue;
            }
            let shared = Arc::new(BridgeShared { hook: Arc::new(Mutex::new(None)) });
            let handler = BridgeClientHandler::new(shared.clone());
            if addr.starts_with("http://") || addr.starts_with("https://") {
                let transport = StreamableHttpClientTransport::from_uri(addr);
                match rmcp::service::serve_client(handler, transport).await {
                    Ok(service) => clients.push((addr.to_string(), service, shared)),
                    Err(e) => {
                        tracing::warn!("外部HTTP服务器不可用({}): {}", addr, e);
                        continue;
                    }
                }
            } else {
                match tokio::net::TcpStream::connect(addr).await {
                    Ok(stream) => match handler.serve(stream).await {
                        Ok(service) => clients.push((addr.to_string(), service, shared)),
                        Err(e) => {
                            tracing::warn!("外部TCP服务器握手失败({}): {}", addr, e);
                            continue;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("外部TCP服务器不可用({}): {}", addr, e);
                        continue;
                    }
                }
            }
        }
        Ok(Self { clients })
    }

    pub async fn new_from_env() -> Result<Self> {
        let mut clients = Vec::new();
        let list = std::env::var("ROBOT_MCP_EXTERNALS").unwrap_or_default();
        for addr in list.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let shared = Arc::new(BridgeShared { hook: Arc::new(Mutex::new(None)) });
            let handler = BridgeClientHandler::new(shared.clone());
            if addr.starts_with("http://") || addr.starts_with("https://") {
                let transport = StreamableHttpClientTransport::from_uri(addr);
                match rmcp::service::serve_client(handler, transport).await {
                    Ok(service) => clients.push((addr.to_string(), service, shared)),
                    Err(e) => {
                        tracing::warn!("外部HTTP服务器不可用({}): {}", addr, e);
                        continue;
                    }
                }
            } else {
                match tokio::net::TcpStream::connect(addr).await {
                    Ok(stream) => match handler.serve(stream).await {
                        Ok(service) => clients.push((addr.to_string(), service, shared)),
                        Err(e) => {
                            tracing::warn!("外部TCP服务器握手失败({}): {}", addr, e);
                            continue;
                        }
                    },
                    Err(e) => {
                        tracing::warn!("外部TCP服务器不可用({}): {}", addr, e);
                        continue;
                    }
                }
            }
        }
        Ok(Self { clients })
    }

    pub async fn list_tools(&self) -> Result<Vec<Tool>> {
        let mut out = Vec::new();
        for (addr, svc, _) in &self.clients {
            let tools = svc.list_all_tools().await?;
            for t in tools {
                let name = format!("ext::{}::{}", addr, t.name);
                let desc = t
                    .description
                    .as_ref()
                    .map(|d| format!("[External {}] {}", addr, d))
                    .unwrap_or_else(|| format!("[External {}]", addr));
                let tool = Tool {
                    name: Cow::Owned(name),
                    title: None,
                    description: Some(Cow::Owned(desc)),
                    input_schema: Arc::new((*t.input_schema).clone()),
                    output_schema: None,
                    annotations: None,
                    icons: None,
                    meta: None,
                };
                out.push(tool);
            }
        }
        Ok(out)
    }

    pub async fn call_external(
        &self,
        namespaced: &str,
        args: Option<serde_json::Value>,
        server_context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult> {
        if let Some((addr, tool)) = parse_external_name(namespaced) {
            for (a, svc, shared) in &self.clients {
                if a == &addr {
                    {
                        let mut lock = shared.hook.lock().await;
                        let ctx = server_context.clone();
                        let func: BridgeFn = Arc::new(move |message: String, _schema: serde_json::Value| {
                            let ctx = ctx.clone();
                            Box::pin(async move {
                                // Ask user for a raw JSON string; external schema embedded in message
                                match ctx.peer.elicit::<BridgeRaw>(message).await {
                                    Ok(opt) => {
                                        if let Some(br) = opt {
                                            // Try parse to JSON
                                            match serde_json::from_str::<serde_json::Value>(&br.raw) {
                                                Ok(v) => Ok(Some(v)),
                                                Err(e) => Err(anyhow::anyhow!(format!("JSON解析失败: {}", e))),
                                            }
                                        } else {
                                            Ok(None)
                                        }
                                    }
                                    Err(e) => Err(anyhow::anyhow!(e.to_string())),
                                }
                            })
                        });
                        *lock = Some(func);
                    }
                    let arguments = match args {
                        Some(v) if v.is_object() => Some(object(v)),
                        _ => None,
                    };
                    let res: CallToolResult = match svc
                        .call_tool(CallToolRequestParam {
                            name: tool.into(),
                            arguments,
                        })
                        .await {
                            Ok(r) => r,
                            Err(e) => {
                                // clear hook on error as well
                                let mut lock = shared.hook.lock().await;
                                *lock = None;
                                return Err(anyhow::anyhow!(e.to_string()));
                            }
                        };
                    {
                        let mut lock = shared.hook.lock().await;
                        *lock = None;
                    }
                    return Ok(res);
                }
            }
            anyhow::bail!("未找到外部服务器: {}", addr);
        }
        anyhow::bail!("不是外部工具命名空间: {}", namespaced);
    }
}

pub fn parse_external_name(name: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = name.split("::").collect();
    if parts.len() >= 3 && parts[0] == "ext" {
        let addr = parts[1].to_string();
        let tool = parts[2..].join("::");
        Some((addr, tool))
    } else {
        None
    }
}
