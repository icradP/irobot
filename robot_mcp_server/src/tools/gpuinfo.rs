use crate::tools::to_object;
use crate::tools::ToolEntry;
use rmcp::{
    ErrorData,
    model::*,
    service::{RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::process::Command;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "GPU 信息查询")]
pub struct GpuInfoRequest {
    #[schemars(description = "GPU 索引（可选），不填则返回所有 GPU 信息")]
    #[serde(default)]
    pub index: Option<u32>,
}

impl rmcp::service::ElicitationSafe for GpuInfoRequest {}

#[derive(Debug, Serialize, Deserialize)]
pub struct GpuInfo {
    pub index: u32,
    pub name: String,
    pub memory_total_mb: u64,
    pub memory_used_mb: u64,
    pub memory_free_mb: u64,
    pub memory_free_percent: f64,
}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(GpuInfoRequest);
    let tool = Tool {
        name: "gpuinfo".into(),
        title: Some("GPU 信息查询".into()),
        description: Some(
            "[Utility] 获取 GPU 显存总量、已用、剩余以及剩余百分比（基于 nvidia-smi）".into(),
        ),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "gpuinfo",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    _context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let mut index: Option<u32> = None;

    if let Some(args) = request {
        if let Ok(parsed) = serde_json::from_value::<GpuInfoRequest>(args.clone()) {
            index = parsed.index;
        } else if let Some(v) = args.get("index") {
            if let Some(i) = v.as_u64() {
                index = Some(i as u32);
            } else if let Some(s) = v.as_str() {
                if let Ok(i) = s.trim().parse::<u32>() {
                    index = Some(i);
                }
            }
        }
    }

    let query_args = [
        "--query-gpu=index,name,memory.total,memory.used,memory.free",
        "--format=csv,noheader,nounits",
    ];

    let output = match Command::new("nvidia-smi").args(&query_args).output().await {
        Ok(o) => o,
        Err(e) => {
            let msg = format!(
                "tool_error\nname=gpuinfo\nmessage=Failed to execute nvidia-smi: {}",
                e
            );
            return Ok(CallToolResult::success(vec![Content::text(msg)]));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let msg = format!(
            "tool_error\nname=gpuinfo\nmessage=nvidia-smi returned error: {}",
            stderr.trim()
        );
        return Ok(CallToolResult::success(vec![Content::text(msg)]));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut infos: Vec<GpuInfo> = Vec::new();

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
        if parts.len() < 5 {
            continue;
        }

        let gpu_index: u32 = match parts[0].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(target) = index {
            if gpu_index != target {
                continue;
            }
        }

        let name = parts[1].to_string();
        let total: u64 = match parts[2].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let used: u64 = match parts[3].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let free: u64 = match parts[4].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };

        if total == 0 {
            continue;
        }

        let free_percent = (free as f64) * 100.0 / (total as f64);

        infos.push(GpuInfo {
            index: gpu_index,
            name,
            memory_total_mb: total,
            memory_used_mb: used,
            memory_free_mb: free,
            memory_free_percent: free_percent,
        });
    }

    if infos.is_empty() {
        let msg = if let Some(i) = index {
            format!(
                "tool_error\nname=gpuinfo\nmessage=No GPU info parsed for index {}",
                i
            )
        } else {
            "tool_error\nname=gpuinfo\nmessage=No GPU info parsed from nvidia-smi output".to_string()
        };
        return Ok(CallToolResult::success(vec![Content::text(msg)]));
    }

    let json = match serde_json::to_string(&infos) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!(
                "tool_error\nname=gpuinfo\nmessage=Failed to serialize GPU info: {}",
                e
            );
            return Ok(CallToolResult::success(vec![Content::text(msg)]));
        }
    };

    Ok(CallToolResult::success(vec![Content::text(json)]))
}
