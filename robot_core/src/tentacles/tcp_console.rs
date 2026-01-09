use crate::core::input_handler::{InputHandler, SourceMetadata, SourceType, TypedInputHandler};
use crate::core::output_handler::{OutputHandler, TypedOutputHandler};
use crate::core::persona::OutputStyle;
use crate::core::router::HandlerMarker;
use crate::utils::{InputEvent, OutputEvent};
use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{error, info, warn};
use uuid::Uuid;

// TCP source type marker
pub struct TcpSource;
impl SourceType for TcpSource {}

/// Type marker for TCP handlers
pub struct TcpHandler;
impl HandlerMarker for TcpHandler {
    const ID: &'static str = "tcp";
}

pub struct TcpSharedState {
    // Map session_id to the sender for that connection
    peers: HashMap<String, mpsc::UnboundedSender<String>>,
}

pub struct TcpInput {
    receiver: Arc<Mutex<mpsc::UnboundedReceiver<InputEvent>>>,
    #[allow(dead_code)]
    state: Arc<RwLock<TcpSharedState>>,
    server_handle: Option<tokio::task::JoinHandle<()>>,
}

pub struct TcpOutput {
    state: Arc<RwLock<TcpSharedState>>,
}

impl TcpInput {
    fn create_metadata() -> SourceMetadata {
        SourceMetadata {
            name: "tcp".to_string(),
            format_hint: "text".to_string(),
            content_field: "content".to_string(),
            description: "User input from raw TCP connection.".to_string(),
        }
    }

    /// Creates a new TCP console listening on the specified port.
    /// Returns a tuple of (TcpInput, TcpOutput, port) sharing the same server state.
    pub async fn new(port: u16) -> Result<(Self, TcpOutput, u16)> {
        let (input_sender, input_receiver) = mpsc::unbounded_channel();
        
        let state = Arc::new(RwLock::new(TcpSharedState {
            peers: HashMap::new(),
        }));

        let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
        let local_addr = listener.local_addr()?;
        let bound_port = local_addr.port();
        info!("TcpConsole server listening on port {}", bound_port);

        let state_clone = state.clone();
        let input_sender_clone = input_sender.clone();

        let server_handle = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok((stream, addr)) => {
                        info!("New TCP connection from: {}", addr);
                        let state = state_clone.clone();
                        let input_sender = input_sender_clone.clone();
                        
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, addr, state, input_sender).await {
                                error!("Error handling connection {}: {}", addr, e);
                            }
                        });
                    }
                    Err(e) => {
                        error!("TCP accept error: {}", e);
                    }
                }
            }
        });

        let input = Self {
            receiver: Arc::new(Mutex::new(input_receiver)),
            state: state.clone(),
            server_handle: Some(server_handle),
        };

        let output = TcpOutput {
            state,
        };

        Ok((input, output, bound_port))
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    addr: SocketAddr,
    state: Arc<RwLock<TcpSharedState>>,
    input_sender: mpsc::UnboundedSender<InputEvent>,
) -> Result<()> {
    let session_id = Uuid::new_v4().to_string();
    info!("Session {} started for {}", session_id, addr);

    let (reader, mut writer) = stream.into_split();
    
    // Channel for sending messages to this client
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();

    // Register peer
    {
        let mut state_guard = state.write().await;
        state_guard.peers.insert(session_id.clone(), tx);
    }

    // Welcome message
    let _ = writer.write_all(b"Welcome to Robot TCP Console!\n").await;
    let _ = writer.write_all(format!("Session ID: {}\n", session_id).as_bytes()).await;

    // Task to write outgoing messages to the socket
    let mut write_task = tokio::spawn(async move {
        while let Some(msg) = rx.recv().await {
            if let Err(e) = writer.write_all(msg.as_bytes()).await {
                warn!("Failed to write to socket: {}", e);
                break;
            }
            // Ensure newline if not present? 
            if !msg.ends_with('\n') {
                let _ = writer.write_all(b"\n").await;
            }
        }
    });

    // Read loop
    let mut buf_reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        tokio::select! {
            bytes_read = buf_reader.read_line(&mut line) => {
                match bytes_read {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let content = line.trim().to_string();
                        if !content.is_empty() {
                            let event = InputEvent {
                                id: Uuid::new_v4(),
                                source: "tcp".to_string(),
                                session_id: Some(session_id.clone()),
                                source_meta: Some(TcpInput::create_metadata()),
                                payload: serde_json::json!({
                                    "content": content,
                                    "timestamp": std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64,
                                }),
                            };
                            
                            // Publish to global event bus for elicitation consumers
                            let _ = crate::utils::event_bus().send(event.clone());

                            // Echo user message to output bus for broadcast
                            let output_echo = OutputEvent {
                                target: "all".to_string(),
                                source: "user".to_string(),
                                session_id: Some(session_id.clone()),
                                content: serde_json::json!({
                                    "type": "user_message",
                                    "content": content,
                                    "timestamp": std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .unwrap_or_default()
                                        .as_millis() as u64,
                                }),
                                style: OutputStyle::Neutral,
                            };
                            let _ = crate::utils::output_bus().send(output_echo);
                            
                            if let Err(_) = input_sender.send(event) {
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!("Error reading from {}: {}", addr, e);
                        break;
                    }
                }
            }
            _ = &mut write_task => {
                // Write task finished (sender closed), so we should close
                break;
            }
        }
    }

    // Cleanup
    info!("Session {} ended for {}", session_id, addr);
    {
        let mut state_guard = state.write().await;
        state_guard.peers.remove(&session_id);
    }
    
    Ok(())
}

#[async_trait]
impl InputHandler for TcpInput {
    async fn poll(&self) -> Result<Option<InputEvent>> {
        let mut receiver = self.receiver.lock().await;
        match receiver.recv().await {
            Some(event) => Ok(Some(event)),
            None => Ok(None),
        }
    }

    fn metadata(&self) -> Option<SourceMetadata> {
        Some(Self::create_metadata())
    }
}

#[async_trait]
impl TypedInputHandler<TcpSource> for TcpInput {
    async fn poll(&self) -> Result<Option<InputEvent>> {
        <Self as InputHandler>::poll(self).await
    }
}

impl Drop for TcpInput {
    fn drop(&mut self) {
        if let Some(handle) = self.server_handle.take() {
            handle.abort();
        }
    }
}

#[async_trait]
impl OutputHandler for TcpOutput {
    async fn emit(&self, event: OutputEvent) -> Result<()> {
        info!("TcpOutput received event: {:?}", event);
        let state = self.state.read().await;
        
        // User requested full debug output of the content, not just the "content" field.
        let message = event.content.to_string();

        // Format output
        let formatted_msg = format!("[{}] {:?}: {}\n", event.source, event.style, message);

        if event.target == "all" {
            for sender in state.peers.values() {
                let _ = sender.send(formatted_msg.clone());
            }
        } else if let Some(sid) = &event.session_id {
            if let Some(sender) = state.peers.get(sid) {
                let _ = sender.send(formatted_msg);
            }
        }

        Ok(())
    }
}

#[async_trait]
impl TypedOutputHandler<TcpSource> for TcpOutput {
    async fn emit(&self, event: OutputEvent) -> Result<()> {
        <Self as OutputHandler>::emit(self, event).await
    }
}

/// A simple TCP client for testing purposes.
/// Connects to the specified address, reads stdin to send, and prints received messages to stdout.
pub async fn run_test_client(addr: &str) -> Result<()> {
    let stream = tokio::net::TcpStream::connect(addr).await?;
    info!("Connected to {}", addr);

    let (reader, mut writer) = stream.into_split();
    
    let mut stdin = tokio::io::BufReader::new(tokio::io::stdin());
    let mut reader = tokio::io::BufReader::new(reader);
    
    let mut input_line = String::new();
    let mut server_line = String::new();

    loop {
        tokio::select! {
            res = stdin.read_line(&mut input_line) => {
                match res {
                    Ok(0) => break, // EOF from stdin
                    Ok(_) => {
                        writer.write_all(input_line.as_bytes()).await?;
                        input_line.clear();
                    }
                    Err(e) => return Err(e.into()),
                }
            }
            res = reader.read_line(&mut server_line) => {
                match res {
                    Ok(0) => break, // Server closed
                    Ok(_) => {
                        print!("{}", server_line);
                        server_line.clear();
                    }
                    Err(e) => return Err(e.into()),
                }
            }
        }
    }
    
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncWriteExt;

    #[tokio::test]
    async fn test_tcp_console() -> Result<()> {
        // Start server on random port
        let (input, output, port) = TcpInput::new(0).await?;
        
        // Spawn client
        let client_handle = tokio::spawn(async move {
            // Allow some time for server to be ready
            tokio::time::sleep(Duration::from_millis(50)).await;
            
            let addr = format!("127.0.0.1:{}", port);
            let stream = tokio::net::TcpStream::connect(&addr).await.expect("Failed to connect");
            let (reader, mut writer) = stream.into_split();
            let mut reader = tokio::io::BufReader::new(reader);
            
            let mut line = String::new();
            
            // Read welcome messages (Welcome + Session ID)
            // We loop until we see Session ID, as they might come in separate lines
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await.expect("Failed to read");
                if n == 0 { panic!("Server closed connection prematurely"); }
                println!("Client received: {}", line.trim());
                if line.contains("Session ID") {
                    break;
                }
            }
            
            // Send message
            writer.write_all(b"Hello Server\n").await.expect("Failed to write");
            
            // Read until we get the expected response
            // We might receive the echo first: "[tcp] Neutral: Hello Server"
            loop {
                line.clear();
                let n = reader.read_line(&mut line).await.expect("Failed to read response");
                if n == 0 { break; }
                println!("Client received: {}", line.trim());
                if line.contains("Response from Test") {
                    break;
                }
            }
            
            Ok::<_, anyhow::Error>(())
        });

        // Server side: wait for input
        let event = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                // Use explicit trait method call to avoid ambiguity
                if let Some(e) = InputHandler::poll(&input).await.unwrap() {
                    return e;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("Timed out waiting for input");

        assert_eq!(event.source, "tcp");
        let content = event.payload["content"].as_str().unwrap();
        assert_eq!(content, "Hello Server");
        
        // Manually emit to output to verify client receives it
        let output_event = OutputEvent {
            target: "all".to_string(),
            source: "test".to_string(),
            session_id: event.session_id.clone(),
            content: serde_json::json!("Response from Test"),
            style: OutputStyle::Neutral,
        };
        // Use explicit trait method call to avoid ambiguity
        OutputHandler::emit(&output, output_event).await?;

        client_handle.await??;
        
        Ok(())
    }
}
