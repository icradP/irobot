use crate::core::input_handler::SourceType;
use crate::utils::OutputEvent;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutputMetadata {
    pub name: String,
    pub format: String,
    pub description: String,
}

#[async_trait]
pub trait OutputHandler {
    async fn emit(&self, event: OutputEvent) -> anyhow::Result<()>;

    /// Return metadata describing this output handler
    fn metadata(&self) -> Option<OutputMetadata> {
        None
    }
}

#[async_trait]
pub trait TypedOutputHandler<T: SourceType>: Send + Sync {
    async fn emit(&self, event: OutputEvent) -> anyhow::Result<()>;
}
