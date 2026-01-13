use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, Deserialize, Clone, Default)]
pub struct AppConfig {
    pub gguf_filename: Option<String>,
    pub force_gpu: Option<bool>,
    pub gpu_id: Option<usize>,
    pub filter_think_default: Option<bool>,
    pub top_k: Option<usize>,
    pub repeat_penalty: Option<f32>,
    pub repeat_last_n: Option<usize>,
}

pub fn load_config() -> AppConfig {
    let path = std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("config.json");
    if let Ok(data) = std::fs::read(&path) {
        if let Ok(cfg) = serde_json::from_slice::<AppConfig>(&data) {
            return cfg;
        }
    }
    AppConfig::default()
}
