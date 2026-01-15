use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::RwLock;
use tokio::task::JoinHandle;
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSummary {
    pub id: String,
    pub name: String,
    pub start_time: DateTime<Utc>,
    pub status: String,
    pub ordinal: u64,
    pub original_prompt: String,
}

pub struct BackgroundTask {
    pub handle: JoinHandle<()>,
    pub name: String,
    pub start_time: DateTime<Utc>,
    pub ordinal: u64,
    pub original_prompt: String,
}

#[derive(Clone)]
pub struct TaskManager {
    tasks: Arc<RwLock<HashMap<String, BackgroundTask>>>,
    counter: Arc<AtomicU64>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            counter: Arc::new(AtomicU64::new(0)),
        }
    }

    pub async fn add_task(
        &self,
        id: String,
        name: String,
        original_prompt: String,
        handle: JoinHandle<()>,
    ) {
        let ordinal = self.counter.fetch_add(1, Ordering::SeqCst) + 1;
        let task = BackgroundTask {
            handle,
            name,
            start_time: Utc::now(),
            ordinal,
            original_prompt,
        };
        self.tasks.write().await.insert(id, task);
    }

    pub async fn remove_task(&self, id: &str) {
        self.tasks.write().await.remove(id);
    }

    pub async fn list_tasks(&self) -> Vec<TaskSummary> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .map(|(id, task)| TaskSummary {
                id: id.clone(),
                name: task.name.clone(),
                start_time: task.start_time,
                status: "Running".to_string(),
                ordinal: task.ordinal,
                original_prompt: task.original_prompt.clone(),
            })
            .collect()
    }

    pub async fn cancel_task(&self, id: &str) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(task) = tasks.remove(id) {
            task.handle.abort();
            return true;
        }
        false
    }
}
