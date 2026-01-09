use crate::core::session::{RobotSession, SessionActor};
use async_trait::async_trait;
use tracing::info;

pub struct WebSession {
    pub inner: RobotSession,
}

#[async_trait]
impl SessionActor for WebSession {
    async fn run(self: Box<Self>) {
        info!("Starting specialized WebSession for {}", self.inner.id);
        // Here we could add web-specific initialization or intercept messages
        self.inner.run_inner().await;
    }
}
