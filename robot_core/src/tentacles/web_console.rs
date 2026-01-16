use crate::core::input_handler::{InputHandler, SourceMetadata, SourceType, TypedInputHandler};
use crate::core::output_handler::{OutputHandler, TypedOutputHandler};
use crate::core::persona::OutputStyle;
use crate::core::router::HandlerMarker;
use crate::utils::{InputEvent, OutputEvent};
use anyhow::Result;
use async_trait::async_trait;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State},
    http::StatusCode,
    response::{Json, Sse},
    routing::{get, post},
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{Mutex, mpsc};
use tokio_stream::StreamExt;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};
use uuid::Uuid;

use md5::{Digest, Md5};
use tokio::sync::RwLock;

// Web source type marker
pub struct WebSource;
impl SourceType for WebSource {}

/// Type marker for web handlers
pub struct WebHandler;
impl HandlerMarker for WebHandler {
    const ID: &'static str = "web";
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileInfo {
    pub md5: String,
    pub path: String,
    pub filename: String,
    pub size: u64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebMessage {
    pub content: String,
    pub timestamp: u64,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub files: Option<Vec<String>>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionResponse {
    pub session_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebResponse {
    pub success: bool,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

pub struct WebInputState {
    pub input_sender: mpsc::UnboundedSender<InputEvent>,
    pub file_registry: Arc<RwLock<HashMap<String, FileInfo>>>,
}

pub struct WebOutputState {
    pub messages: Arc<Mutex<Vec<OutputEvent>>>,
    pub subscribers: Arc<Mutex<HashMap<String, HashMap<Uuid, mpsc::UnboundedSender<OutputEvent>>>>>,
}

pub struct WebInput {
    pub receiver: Arc<Mutex<mpsc::UnboundedReceiver<InputEvent>>>,
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl WebInput {
    fn create_metadata() -> SourceMetadata {
        SourceMetadata {
            name: "web".to_string(),
            format_hint: "structured".to_string(),
            content_field: "content".to_string(),
            description:
                "User input from web chat interface. Includes timestamp and structured metadata."
                    .to_string(),
        }
    }

    pub async fn new(port: u16) -> Result<Self> {
        let (input_sender, input_receiver) = mpsc::unbounded_channel();
        let input_state = WebInputState {
            input_sender,
            file_registry: Arc::new(RwLock::new(HashMap::new())),
        };

        let app = Router::new()
            .route("/api/send/{session_id}", post(send_message))
            .route("/api/session", post(create_session))
            .route("/api/upload", post(upload_file))
            .route("/api/check_file", post(check_file))
            .route("/health", get(health_check))
            .layer(DefaultBodyLimit::max(1024 * 1024 * 1024)) // 1GB limit
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any),
            )
            .with_state(Arc::new(input_state));

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
        info!("WebInput server listening on port {}", port);

        let server_handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                error!("WebInput server error: {}", e);
            }
        });

        Ok(Self {
            receiver: Arc::new(Mutex::new(input_receiver)),
            server_handle: Some(server_handle),
        })
    }
}

#[async_trait]
impl InputHandler for WebInput {
    async fn poll(&self) -> Result<Option<InputEvent>> {
        info!("web input waiting for message");
        let mut receiver = self.receiver.lock().await;
        match receiver.recv().await {
            Some(event) => {
                info!("web input received: {:?}", event);
                Ok(Some(event))
            }
            None => {
                warn!("web input channel closed");
                Ok(None)
            }
        }
    }

    fn metadata(&self) -> Option<SourceMetadata> {
        Some(Self::create_metadata())
    }
}

#[async_trait]
impl TypedInputHandler<WebSource> for WebInput {
    async fn poll(&self) -> Result<Option<InputEvent>> {
        <Self as InputHandler>::poll(self).await
    }
}

impl Drop for WebInput {
    fn drop(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }
}

pub struct WebOutput {
    pub state: Arc<WebOutputState>,
    pub server_handle: Option<tokio::task::JoinHandle<()>>,
}

impl WebOutput {
    pub async fn new(port: u16) -> Result<Self> {
        let state = Arc::new(WebOutputState {
            messages: Arc::new(Mutex::new(Vec::new())),
            subscribers: Arc::new(Mutex::new(HashMap::new())),
        });

        let app = Router::new()
            .route("/api/messages/", get(get_messages)) // 获取所有消息
            .route("/api/messages/{session_id}", get(get_messages_by_session)) // 获取指定会话的消息
            .route("/api/subscribe", get(subscribe_to_messages)) //获取主动通知消息
            .route("/health", get(health_check))
            .layer(
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_headers(Any),
            )
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
        info!("WebOutput server listening on port {}", port);

        let server_handle = tokio::spawn(async move {
            if let Err(e) = axum::serve(listener, app).await {
                error!("WebOutput server error: {}", e);
            }
        });

        Ok(Self {
            state,
            server_handle: Some(server_handle),
        })
    }
}

#[async_trait]
impl OutputHandler for WebOutput {
    async fn emit(&self, event: OutputEvent) -> Result<()> {
        info!("web output emitting message");

        // Store the message
        {
            let mut messages = self.state.messages.lock().await;
            messages.push(event.clone());

            // Keep only the last 100 messages to prevent memory growth
            if messages.len() > 100 {
                messages.remove(0);
            }
        }

        // Notify subscribers
        {
            let mut subscribers = self.state.subscribers.lock().await;

            if event.target == "all" {
                // Broadcast to everyone
                for map in subscribers.values_mut() {
                    let mut to_remove = Vec::new();
                    for (id, sender) in map.iter() {
                        if sender.send(event.clone()).is_err() {
                            to_remove.push(*id);
                        }
                    }
                    for id in to_remove {
                        map.remove(&id);
                    }
                }
            } else if let Some(sid) = &event.session_id {
                // Send only to session subscribers
                if let Some(map) = subscribers.get_mut(sid) {
                    let mut to_remove = Vec::new();
                    for (id, sender) in map.iter() {
                        if sender.send(event.clone()).is_err() {
                            to_remove.push(*id);
                        }
                    }
                    for id in to_remove {
                        map.remove(&id);
                    }
                }
            }
        }

        Ok(())
    }
}

#[async_trait]
impl TypedOutputHandler<WebSource> for WebOutput {
    async fn emit(&self, event: OutputEvent) -> Result<()> {
        <Self as OutputHandler>::emit(self, event).await
    }
}

// HTTP handlers for WebInput
async fn send_message(
    State(state): State<Arc<WebInputState>>,
    Json(message): Json<WebMessage>,
) -> Result<Json<WebResponse>, StatusCode> {
    let mut combined_content = message.content.clone();
    if let Some(files) = &message.files {
        if !files.is_empty() {
            combined_content.push_str("\n\n[System Note: User uploaded files]\n");
            for file in files {
                combined_content.push_str(&format!("- {}\n", file));
            }
        }
    }

    let input_event = InputEvent {
        id: Uuid::new_v4(),
        source: "web".to_string(),
        session_id: message.session_id.clone(),
        source_meta: Some(SourceMetadata {
            name: "web".to_string(),
            format_hint: "structured".to_string(),
            content_field: "content".to_string(),
            description: "User input from web chat interface.".to_string(),
        }),
        payload: serde_json::json!({
            "content": combined_content,
            "timestamp": message.timestamp,
            "files": message.files
        }),
    };

    // Echo user message to output bus for broadcast
    let output_echo = OutputEvent {
        target: "all".to_string(),
        source: "user".to_string(),
        session_id: message.session_id.clone(),
        content: serde_json::json!({
            "type": "user_message",
            "content": message.content,
            "timestamp": message.timestamp,
            "files": message.files
        }),
        style: OutputStyle::Neutral.to_string(),
    };
    let _ = crate::utils::output_bus().send(output_echo);

    // publish to global event bus for elicitation consumers
    let _ = crate::utils::event_bus().send(input_event.clone());
    match state.input_sender.send(input_event) {
        Ok(_) => Ok(Json(WebResponse {
            success: true,
            message: "Message sent successfully".to_string(),
            data: None,
        })),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

// HTTP handlers for WebOutput
async fn get_messages(State(state): State<Arc<WebOutputState>>) -> Json<Vec<OutputEvent>> {
    let messages = state.messages.lock().await;
    Json(messages.clone())
}

async fn get_messages_by_session(
    State(state): State<Arc<WebOutputState>>,
    Path(session_id): Path<String>,
) -> Json<Vec<OutputEvent>> {
    let messages = state.messages.lock().await;
    let filtered: Vec<OutputEvent> = messages
        .iter()
        .filter(|msg| msg.session_id.as_deref() == Some(&session_id))
        .cloned()
        .collect();
    Json(filtered)
}

#[derive(Deserialize)]
struct SubscribeQuery {
    session_id: Option<String>,
}

async fn subscribe_to_messages(
    State(state): State<Arc<WebOutputState>>,
    Query(q): Query<SubscribeQuery>,
) -> Sse<impl Stream<Item = Result<axum::response::sse::Event, Infallible>>> {
    let subscriber_id = Uuid::new_v4();
    let (sender, receiver) = mpsc::unbounded_channel();

    {
        let mut subscribers = state.subscribers.lock().await;
        let sid = q.session_id.clone().unwrap_or_else(|| "web".to_string());
        subscribers
            .entry(sid)
            .or_insert_with(HashMap::new)
            .insert(subscriber_id, sender);
    }

    let stream = tokio_stream::wrappers::UnboundedReceiverStream::new(receiver).map(|event| {
        let json = serde_json::to_string(&event).unwrap_or_default();
        Ok(axum::response::sse::Event::default().data(json))
    });

    Sse::new(stream)
}

async fn health_check() -> Json<WebResponse> {
    Json(WebResponse {
        success: true,
        message: "Service is healthy".to_string(),
        data: None,
    })
}

async fn create_session() -> Json<CreateSessionResponse> {
    Json(CreateSessionResponse {
        session_id: Uuid::new_v4().to_string(),
    })
}

#[derive(Deserialize)]
struct CheckFileRequest {
    md5: String,
}

#[derive(Serialize)]
struct CheckFileResponse {
    exists: bool,
    file: Option<FileInfo>,
}

async fn check_file(
    State(state): State<Arc<WebInputState>>,
    Json(req): Json<CheckFileRequest>,
) -> Json<CheckFileResponse> {
    let registry = state.file_registry.read().await;
    if let Some(info) = registry.get(&req.md5) {
        Json(CheckFileResponse {
            exists: true,
            file: Some(info.clone()),
        })
    } else {
        Json(CheckFileResponse {
            exists: false,
            file: None,
        })
    }
}

async fn upload_file(
    State(state): State<Arc<WebInputState>>,
    mut multipart: Multipart,
) -> Result<Json<WebResponse>, StatusCode> {
    let mut file_paths = Vec::new();
    let upload_dir = PathBuf::from("uploads");
    if !upload_dir.exists() {
        tokio::fs::create_dir_all(&upload_dir).await.map_err(|e| {
            error!("Failed to create upload dir: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;
    }

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        error!("Multipart error: {}", e);
        StatusCode::BAD_REQUEST
    })? {
        let file_name = field.file_name().unwrap_or("unknown_file").to_string();
        let safe_name: String = file_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '.' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let saved_filename = format!("{}_{}", Uuid::new_v4(), safe_name);
        let path = upload_dir.join(&saved_filename);

        let data = field.bytes().await.map_err(|e| {
            error!("Bytes read error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        // Calculate MD5
        let size = data.len() as u64;
        let mut hasher = Md5::new();
        hasher.update(&data);
        let hash = hasher.finalize();
        let md5_hex = hex::encode(hash);

        // Check registry first
        {
            let registry = state.file_registry.read().await;
            if let Some(info) = registry.get(&md5_hex) {
                file_paths.push(info.path.clone());
                continue;
            }
        }

        tokio::fs::write(&path, data).await.map_err(|e| {
            error!("File write error: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

        let abs_path = std::fs::canonicalize(&path).unwrap_or(path);
        let uri = format!("file://{}", abs_path.display());
        file_paths.push(uri.clone());

        // Update registry
        {
            let mut registry = state.file_registry.write().await;
            registry.insert(
                md5_hex.clone(),
                FileInfo {
                    md5: md5_hex,
                    path: uri,
                    filename: safe_name,
                    size,
                },
            );
        }
    }

    Ok(Json(WebResponse {
        success: true,
        message: "Upload successful".to_string(),
        data: Some(serde_json::json!({ "files": file_paths })),
    }))
}
