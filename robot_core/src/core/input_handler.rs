use crate::utils::InputEvent;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

// Marker trait for source types
pub trait SourceType: Send + Sync + 'static {}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceMetadata {
    pub name: String,
    pub format_hint: String,
    pub content_field: String,
    pub description: String,
}

#[async_trait]
pub trait InputHandler {
    async fn poll(&self) -> anyhow::Result<Option<InputEvent>>;

    /// Return metadata describing this input source
    fn metadata(&self) -> Option<SourceMetadata> {
        None
    }
}

#[async_trait]
pub trait TypedInputHandler<T: SourceType>: Send + Sync {
    async fn poll(&self) -> anyhow::Result<Option<InputEvent>>;
}

pub struct NullInput;

#[async_trait]
impl InputHandler for NullInput {
    async fn poll(&self) -> anyhow::Result<Option<InputEvent>> {
        Ok(None)
    }
}
