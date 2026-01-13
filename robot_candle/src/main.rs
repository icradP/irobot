mod qwen3;
mod utils;
mod config;

use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
    routing::post,
    Router,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::task;

use crate::qwen3::{Qwen3Engine, GenerationParams};
use crate::config::load_config;

#[derive(Clone)]
struct AppState {
    engine: Arc<Mutex<Qwen3Engine>>,
}

#[derive(Deserialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<Message>,
    temperature: Option<f64>,
    top_p: Option<f64>,
    max_tokens: Option<usize>,
    filter_think: Option<bool>,
    top_k: Option<usize>,
    repeat_penalty: Option<f32>,
    repeat_last_n: Option<usize>,
}

#[derive(Deserialize, Serialize, Clone)]
struct Message {
    role: String,
    content: String,
}

fn format_qwen_messages(msgs: &[Message]) -> String {
    let mut s = String::new();
    for m in msgs {
        s.push_str("<|im_start|>");
        s.push_str(&m.role);
        s.push('\n');
        s.push_str(&m.content);
        s.push('\n');
        s.push_str("<|im_end|>\n");
    }
    s.push_str("<|im_start|>assistant\n");
    s
}

#[derive(Serialize)]
struct ChatCompletionResponse {
    id: String,
    object: String,
    created: u64,
    model: String,
    choices: Vec<Choice>,
    usage: Usage,
}

#[derive(Serialize)]
struct Choice {
    index: usize,
    message: Message,
    finish_reason: String,
}

#[derive(Serialize)]
struct Usage {
    prompt_tokens: usize,
    completion_tokens: usize,
    total_tokens: usize,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();

    let cfg = load_config();
    if let Some(gpu_id) = cfg.gpu_id {
        // 避免使用 set_var（unsafe），通过传递环境变量由外层启动控制更安全。
        // 这里仅提示当前配置，实际选择在 utils::device 中根据环境变量读取。
        println!("Configured GPU_ID from config: {}", gpu_id);
    }
    let cpu_only = if cfg.force_gpu.unwrap_or(false) {
        false
    } else {
        std::env::var("CPU_ONLY").ok().map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false)
    };

    println!("Loading Qwen3 model...");
    let engine = Qwen3Engine::new(cpu_only)?;
    println!("Qwen3 model loaded successfully.");

    let state = AppState {
        engine: Arc::new(Mutex::new(engine)),
    };

    let app = Router::new()
        .route("/v1/chat/completions", post(chat_completions))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await?;
    println!("Server running on http://0.0.0.0:3000");
    axum::serve(listener, app).await?;

    Ok(())
}

async fn chat_completions(
    State(state): State<AppState>,
    Json(payload): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    // For this simple example, we only take the last user message as the prompt.
    // A production implementation should format the entire conversation history according to templates.
    let last_user_msg = payload.messages.iter().filter(|m| m.role == "user").last();
    let prompt = match last_user_msg {
        Some(msg) => msg.content.clone(),
        None => return (StatusCode::BAD_REQUEST, "No user message found").into_response(),
    };

    // Run inference in a blocking task to avoid blocking the async runtime
    let engine = state.engine.clone();
    let result = task::spawn_blocking(move || {
        let mut engine = engine.lock().unwrap();
        let cfg = load_config();
        let prompt_str = format_qwen_messages(&payload.messages);
        let params = GenerationParams {
            temperature: payload.temperature.unwrap_or(0.8),
            top_p: payload.top_p.unwrap_or(0.8),
            top_k: payload.top_k.or_else(|| cfg.top_k),
            repeat_penalty: payload.repeat_penalty.or(cfg.repeat_penalty).unwrap_or(1.0),
            repeat_last_n: payload.repeat_last_n.or(cfg.repeat_last_n).unwrap_or(64),
            filter_think: payload.filter_think.or(cfg.filter_think_default).unwrap_or(true),
            max_tokens: payload.max_tokens.unwrap_or(1024),
        };
        engine.generate(&prompt_str, params)
    }).await;

    match result {
        Ok(Ok(response_text)) => {
            let created = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let response = ChatCompletionResponse {
                id: format!("chatcmpl-{}", created),
                object: "chat.completion".to_string(),
                created,
                model: payload.model,
                choices: vec![Choice {
                    index: 0,
                    message: Message {
                        role: "assistant".to_string(),
                        content: response_text,
                    },
                    finish_reason: "stop".to_string(),
                }],
                usage: Usage {
                    prompt_tokens: 0, 
                    completion_tokens: 0,
                    total_tokens: 0,
                },
            };
            Json(response).into_response()
        }
        Ok(Err(e)) => {
            println!("Generation error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Generation failed").into_response()
        }
        Err(e) => {
            println!("Task join error: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal task error").into_response()
        }
    }
}
