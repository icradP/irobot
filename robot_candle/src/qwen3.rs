use anyhow::{Error, Result};
use candle::{Device, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use candle_transformers::utils::apply_repeat_penalty;
use candle_transformers::models::quantized_qwen3::ModelWeights as Qwen3;
use candle::quantized::gguf_file;
use hf_hub::api::sync::Api;
use tokenizers::Tokenizer;
use crate::utils;

#[derive(Clone)]
pub struct GenerationParams {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: Option<usize>,
    pub repeat_penalty: f32,
    pub repeat_last_n: usize,
    pub filter_think: bool,
    pub max_tokens: usize,
}
pub struct Qwen3Engine {
    model: Qwen3,
    tokenizer: Tokenizer,
    device: Device,
    eos_tokens: Vec<u32>,
}

impl Qwen3Engine {
    pub fn new(cpu: bool) -> Result<Self> {
        let device = utils::device(cpu)?;
        if device.is_cuda() {
            println!("Using CUDA GPU for inference");
        } else {
            println!("Using CPU for inference");
        }
        let gguf_dir = std::env::current_dir()?.join("gguf");
        let local_tokenizer = gguf_dir.join("qwen3-14B-tokenizer.json");
        let tokenizer = if local_tokenizer.exists() {
            println!("Loading local tokenizer from {:?}", local_tokenizer);
            Tokenizer::from_file(&local_tokenizer).map_err(Error::msg)?
        } else {
            println!("本地 gguf/tokenizer.json 不存在，尝试从 HuggingFace 下载 (Qwen/Qwen3-14B)...");
            let api = Api::new()?;
            let repo = api.model("Qwen/Qwen3-14B".to_string());
            let downloaded = repo.get("qwen3-14B-tokenizer.json")?;
            if !gguf_dir.exists() {
                std::fs::create_dir_all(&gguf_dir)?;
            }
            std::fs::copy(&downloaded, &local_tokenizer)?;
            println!("已保存 tokenizer.json 到 {:?}", local_tokenizer);
            Tokenizer::from_file(&local_tokenizer).map_err(Error::msg)?
        };

        let env_path = std::env::var("GGUF_FILENAME").ok();
        let gguf_path = if let Some(p) = env_path {
             std::path::PathBuf::from(p)
        } else {
            let local_dir = gguf_dir.clone();
            let candidates = [
                "Qwen3-14B-Q4_K_M.gguf",
                "Qwen3-14B-Q4_K_M",
                "Qwen3-14B-Q4_K_M-GGUF",
                "Qwen3-14B-Q4_K_M-GGUF.gguf",
            ];
            let found = candidates
                .iter()
                .map(|f| local_dir.join(f))
                .find(|p| p.exists());
            if let Some(p) = found {
                p
            } else {
                return Err(anyhow::anyhow!(
                    "未在 gguf/ 目录找到 GGUF 文件，请放置下列之一：Qwen3-14B-Q4_K_M(.gguf) 或 Qwen3-14B-Q4_K_M-GGUF(.gguf)"
                ));
            }
        };

        println!("Loading Qwen3 GGUF from {:?}", gguf_path);
        let start = std::time::Instant::now();
        let mut file = std::fs::File::open(&gguf_path)?;
        let content = gguf_file::Content::read(&mut file)?;
        let mut total_size_in_bytes = 0usize;
        for (_, tensor) in content.tensor_infos.iter() {
            let elem_count = tensor.shape.elem_count();
            total_size_in_bytes += elem_count * tensor.ggml_dtype.type_size() / tensor.ggml_dtype.block_size();
        }
        println!(
            "loaded {:?} tensors ({:.2}MB) in {:.2}s",
            content.tensor_infos.len(),
            total_size_in_bytes as f64 / 1e6,
            start.elapsed().as_secs_f32(),
        );
        let model = Qwen3::from_gguf(content, &mut file, &device)?;
        println!("model built");

        let mut eos_tokens = Vec::new();
        if let Some(id) = tokenizer.token_to_id("<|im_end|>") { eos_tokens.push(id); }
        if let Some(id) = tokenizer.token_to_id("<|endoftext|>") { eos_tokens.push(id); }

        Ok(Self {
            model,
            tokenizer,
            device,
            eos_tokens,
        })
    }

    pub fn generate(&mut self, prompt_str: &str, params: GenerationParams) -> Result<String> {
        self.model.clear_kv_cache();
        let seed = 299792458u64;
        let sampling = if params.temperature <= 0.0 {
            Sampling::ArgMax
        } else {
            match (params.top_k, Some(params.top_p)) {
                (None, None) => Sampling::All { temperature: params.temperature },
                (Some(k), None) => Sampling::TopK { k, temperature: params.temperature },
                (None, Some(p)) => Sampling::TopP { p, temperature: params.temperature },
                (Some(k), Some(p)) => Sampling::TopKThenTopP { k, p, temperature: params.temperature },
            }
        };
        let mut logits_processor = LogitsProcessor::from_sampling(seed, sampling);
        let tokens = self.tokenizer.encode(prompt_str, true).map_err(Error::msg)?;
        let mut tokens = tokens.get_ids().to_vec();
        let to_sample = params.max_tokens.saturating_sub(1);
        let mut all_tokens = vec![];
        let mut output_tokens = Vec::new();
        let start_prompt_processing = std::time::Instant::now();

        let mut next_token = {
            let input = Tensor::new(&tokens[..], &self.device)?.unsqueeze(0)?;
            let logits = self.model.forward(&input, 0)?;
            let logits = logits.squeeze(0)?;
            logits_processor.sample(&logits)?
        };
        let prompt_dt = start_prompt_processing.elapsed();

        all_tokens.push(next_token);
        output_tokens.push(next_token);

        let eos_token = self.tokenizer.get_vocab(true).get("<|im_end|>").copied();
        let start_post_prompt = std::time::Instant::now();
        let mut sampled = 0usize;
        for index in 0..to_sample {
            let input = Tensor::new(&[next_token], &self.device)?.unsqueeze(0)?;
            let mut logits = self.model.forward(&input, tokens.len() + index)?;
            let logits = logits.squeeze(0)?;
            let logits = if params.repeat_penalty == 1.0 {
                logits
            } else {
                let start_at = all_tokens.len().saturating_sub(params.repeat_last_n);
                apply_repeat_penalty(&logits, params.repeat_penalty, &all_tokens[start_at..])?
            };
            next_token = logits_processor.sample(&logits)?;
            all_tokens.push(next_token);
            output_tokens.push(next_token);
            sampled += 1;
            if let Some(eos) = eos_token {
                if next_token == eos { break; }
            } else if self.eos_tokens.contains(&next_token) {
                break;
            }
        }
        let dt = start_post_prompt.elapsed();
        println!(
            "prompt tokens processed: {} ({:.2} token/s)",
            tokens.len(),
            tokens.len() as f64 / prompt_dt.as_secs_f64(),
        );
        println!(
            "generated tokens: {} ({:.2} token/s)",
            sampled,
            sampled as f64 / dt.as_secs_f64(),
        );
        
        let mut decoded = self.tokenizer.decode(&output_tokens, true).map_err(Error::msg)?;
        if params.filter_think {
            if let Some(start) = decoded.find("<think>") {
                if let Some(end) = decoded[start..].find("</think>") {
                    let end_idx = start + end + "</think>".len();
                    let mut s = String::new();
                    s.push_str(&decoded[..start]);
                    s.push_str(&decoded[end_idx..]);
                    decoded = s;
                } else {
                    decoded = decoded.replace("<think>", "");
                }
            }
        }
        Ok(decoded.trim().to_string())
    }
}
