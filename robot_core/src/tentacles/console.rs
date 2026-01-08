use crate::core::input_handler::{InputHandler, SourceMetadata, SourceType, TypedInputHandler};
use crate::core::output_handler::{OutputHandler, TypedOutputHandler};
use crate::core::router::HandlerMarker;
use crate::core::stdin_manager::StdinManager;
use crate::utils::{InputEvent, OutputEvent};
use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::{broadcast, Mutex};
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

// Console source type marker
pub struct ConsoleSource;
impl SourceType for ConsoleSource {}

pub struct ConsoleInput {
    receiver: Arc<Mutex<broadcast::Receiver<String>>>,
}

/// Type marker for console handlers
pub struct ConsoleHandler;
impl HandlerMarker for ConsoleHandler {
    const ID: &'static str = "console";
}

impl ConsoleInput {
    pub fn new(manager: &StdinManager) -> Self {
        Self {
            receiver: Arc::new(Mutex::new(manager.subscribe())),
        }
    }

    fn create_metadata() -> SourceMetadata {
        SourceMetadata {
            name: "console".to_string(),
            format_hint: "plain_text".to_string(),
            content_field: "line".to_string(),
            description: "User input from console/terminal. Raw text from stdin.".to_string(),
        }
    }
}

#[async_trait]
impl InputHandler for ConsoleInput {
    async fn poll(&self) -> Result<Option<InputEvent>> {
        info!("console input waiting");
        let mut rx = self.receiver.lock().await;
        match rx.recv().await {
            Ok(line) => {
                info!("console input line: {}", line);
                let payload = serde_json::json!({ "line": line });
                Ok(Some(InputEvent {
                    id: Uuid::new_v4(),
                    source: "console".into(),
                    source_meta: Some(Self::create_metadata()),
                    payload,
                }))
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("console input eof");
                Ok(None)
            }
            Err(broadcast::error::RecvError::Lagged(_)) => {
                // If lagged, just try again (recv returns Lagged error once, next call returns latest)
                // But we are in a poll loop, so just return None (retry) or recurse?
                // Returning None with Ok will cause the loop to sleep briefly.
                Ok(None)
            }
        }
    }

    fn metadata(&self) -> Option<SourceMetadata> {
        Some(Self::create_metadata())
    }
}

#[async_trait]
impl TypedInputHandler<ConsoleSource> for ConsoleInput {
    async fn poll(&self) -> Result<Option<InputEvent>> {
        <Self as InputHandler>::poll(self).await
    }
}

pub struct ConsoleOutput;

#[async_trait]
impl OutputHandler for ConsoleOutput {
    async fn emit(&self, event: OutputEvent) -> Result<()> {
        info!("console output emitting");
        println!("{}", serde_json::to_string(&event)?);
        Ok(())
    }
}

#[async_trait]
impl TypedOutputHandler<ConsoleSource> for ConsoleOutput {
    async fn emit(&self, event: OutputEvent) -> Result<()> {
        <Self as OutputHandler>::emit(self, event).await
    }
}
