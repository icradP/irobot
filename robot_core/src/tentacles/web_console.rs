use crate::core::input_handler::{InputHandler, SourceMetadata, SourceType, TypedInputHandler};
use crate::core::output_handler::{OutputHandler, TypedOutputHandler};
use crate::core::persona::OutputStyle;
use crate::core::router::HandlerMarker;
use crate::utils::{InputEvent, OutputEvent};
use anyhow::Result;
use async_trait::async_trait;
use axum::{
    extract::{State, Query},
    http::StatusCode,
    response::{Json, Sse},
    routing::{get, post},
    Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tokio_stream::StreamExt;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info, warn};
use uuid::Uuid;

// Web source type marker
pub struct WebSource;
impl SourceType for WebSource {}

/// Type marker for web handlers
pub struct WebHandler;
impl HandlerMarker for WebHandler {
    const ID: &'static str = "web";
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebMessage {
    pub content: String,
    pub timestamp: u64,
    #[serde(default)]
    pub session_id: Option<String>,
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
        let input_state = WebInputState { input_sender };

        let app = Router::new()
            .route("/api/send", post(send_message))
            .route("/health", get(health_check))
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
            .route("/api/messages", get(get_messages))
            .route("/api/subscribe", get(subscribe_to_messages))
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

        // Notify all subscribers (Broadcast to everyone)
        {
            let mut subscribers = self.state.subscribers.lock().await;
            
            // Iterate over all session buckets
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
            "content": message.content,
            "timestamp": message.timestamp
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
            "timestamp": message.timestamp
        }),
        style: OutputStyle::Neutral,
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
