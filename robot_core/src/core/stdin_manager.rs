use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex};
use std::sync::Arc;
use tracing::{info, error};

pub struct StdinManager {
    broadcast_tx: broadcast::Sender<String>,
    claim_tx: mpsc::Sender<oneshot::Sender<String>>,
    claim_rx: Arc<Mutex<mpsc::Receiver<oneshot::Sender<String>>>>,
}

impl StdinManager {
    pub fn new() -> Arc<Self> {
        let (broadcast_tx, _) = broadcast::channel(100);
        let (claim_tx, claim_rx) = mpsc::channel::<oneshot::Sender<String>>(16);
        let claim_rx = Arc::new(Mutex::new(claim_rx));
        
        let manager = Arc::new(Self { 
            broadcast_tx: broadcast_tx.clone(),
            claim_tx,
            claim_rx: claim_rx.clone(),
        });
        
        let broadcast_tx_clone = broadcast_tx.clone();
        let claim_rx_clone = claim_rx.clone();
        
        fn deliver_line(
            line: String,
            broadcast_tx: &broadcast::Sender<String>,
            claim_rx: &Arc<Mutex<mpsc::Receiver<oneshot::Sender<String>>>>,
        ) {
            if let Ok(mut rx_guard) = claim_rx.try_lock() {
                match rx_guard.try_recv() {
                    Ok(responder) => {
                        let _ = responder.send(line);
                    }
                    Err(_) => {
                        let _ = broadcast_tx.send(line);
                    }
                }
            } else {
                let _ = broadcast_tx.send(line);
            }
        }
        
        // Spawn the stdin reader task
        tokio::spawn(async move {
            let stdin = tokio::io::stdin();
            let mut reader = BufReader::new(stdin);
            let mut line = String::new();
            
            loop {
                line.clear();
                match reader.read_line(&mut line).await {
                    Ok(0) => break, // EOF
                    Ok(_) => {
                        let trimmed = line.trim().to_string();
                        if trimmed.is_empty() {
                            continue;
                        }
                        deliver_line(trimmed, &broadcast_tx_clone, &claim_rx_clone);
                    }
                    Err(e) => {
                        error!("Stdin read error: {}", e);
                        break;
                    }
                }
            }
        });
        
        manager
    }
    
    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.broadcast_tx.subscribe()
    }
    
    pub async fn claim_next_line(&self) -> String {
        let (tx, rx) = oneshot::channel();
        if let Err(e) = self.claim_tx.send(tx).await {
            error!("Failed to register claim: {}", e);
            return String::new();
        }
        rx.await.unwrap_or_default()
    }

    pub async fn submit_line(&self, line: String) {
        if line.trim().is_empty() {
            return;
        }
        if let Ok(mut rx_guard) = self.claim_rx.lock().await.try_recv() {
            let _ = rx_guard.send(line);
        } else {
            let _ = self.broadcast_tx.send(line);
        }
    }
}
