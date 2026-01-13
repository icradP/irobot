use std::collections::HashSet;

use candle::utils::{cuda_is_available, metal_is_available};
use candle::{Device, Result, Tensor};

pub fn device(cpu: bool) -> Result<Device> {
    if cpu {
        Ok(Device::Cpu)
    } else if cuda_is_available() {
        let gpu_idx = std::env::var("GPU_ID")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0);
        Ok(Device::new_cuda(gpu_idx)?)
    } else if metal_is_available() {
        Ok(Device::new_metal(0)?)
    } else {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            println!(
                "Running on CPU, to run on GPU(metal), build this example with `--features metal`"
            );
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            println!("Running on CPU, to run on GPU, build this example with `--features cuda`");
        }
        Ok(Device::Cpu)
    }
}

pub fn hub_load_safetensors(
    repo: &hf_hub::api::sync::ApiRepo,
    json_file: &str,
) -> Result<Vec<std::path::PathBuf>> {
    let json_file = repo.get(json_file).map_err(candle::Error::wrap)?;
    let json_file = std::fs::File::open(json_file)?;
    let json: serde_json::Value =
        serde_json::from_reader(&json_file).map_err(candle::Error::wrap)?;
    let weight_map = match json.get("weight_map") {
        None => candle::bail!("no weight map in {json_file:?}"),
        Some(serde_json::Value::Object(map)) => map,
        Some(_) => candle::bail!("weight map in {json_file:?} is not a map"),
    };
    let mut safetensors_files = HashSet::new();
    for value in weight_map.values() {
        if let Some(file) = value.as_str() {
            safetensors_files.insert(file.to_string());
        }
    }
    let safetensors_files = safetensors_files
        .iter()
        .map(|v| repo.get(v).map_err(candle::Error::wrap))
        .collect::<Result<Vec<_>>>()?;
    Ok(safetensors_files)
}

pub fn download_glm4_gguf(filename: Option<String>) -> anyhow::Result<std::path::PathBuf> {
    let fname = filename.unwrap_or_else(|| "THUDM_GLM-4-9B-0414-Q6_K_L.gguf".to_string());
    
    // Check local gguf directory first
    let local_path = std::env::current_dir()?.join("gguf").join(&fname);
    if local_path.exists() {
        println!("Found local GGUF model: {:?}", local_path);
        return Ok(local_path);
    }
    
    // Check if it is a direct path
    let path = std::path::Path::new(&fname);
    if path.exists() {
         println!("Found local GGUF model: {:?}", path);
         return Ok(path.to_path_buf());
    }

    println!("Local GGUF model not found, attempting to download from HF Hub...");
    let api = hf_hub::api::sync::Api::new()?;
    let repo = api.repo(hf_hub::Repo::with_revision(
        "bartowski/THUDM_GLM-4-9B-0414-GGUF".to_string(),
        hf_hub::RepoType::Model,
        "main".to_string(),
    ));
    let path = repo.get(&fname)?;
    Ok(path)
}
