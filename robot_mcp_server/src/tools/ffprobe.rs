use crate::tools::ToolEntry;
use crate::tools::to_object;
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
#[schemars(description = "ffprobe 分析工具")]
pub struct FFProbeRequest {
    #[schemars(description = "输入文件路径或 URI（例如 file:///path/to/video.mp4）")]
    pub input: String,
}
impl rmcp::service::ElicitationSafe for FFProbeRequest {}

fn clean_path(path: &str) -> String {
    if let Some(stripped) = path.strip_prefix("file://") {
        stripped.to_string()
    } else {
        path.to_string()
    }
}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(FFProbeRequest);
    let tool = Tool {
        name: "ffprobe_tool".into(),
        title: Some("FFprobe 媒体分析工具".into()),
        description: Some("使用 ffprobe 分析媒体文件信息".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "ffprobe_tool",
        tool,
        handler: Arc::new(|req, ctx, _state| Box::pin(handle(req, ctx))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let args: FFProbeRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        let params = match context
            .peer
            .elicit::<FFProbeRequest>("请输入要分析的媒体文件路径或 URI".to_string())
            .await
        {
            Ok(Some(params)) => params,
            Ok(None) => return Err(ErrorData::invalid_params("未提供参数", None)),
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        };
        params
    };

    let input_path = clean_path(&args.input);
    analyze_media(&input_path).await
}

async fn analyze_media(input: &str) -> Result<CallToolResult, ErrorData> {
    let output = Command::new("ffprobe")
        .args(["-hide_banner", "-show_format", "-show_streams", input])
        .output()
        .await
        .map_err(|e| ErrorData::internal_error(format!("Failed to execute ffprobe: {}", e), None))?;

    if !output.status.success() {
        return Err(ErrorData::internal_error(
            format!(
                "FFprobe failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ),
            None,
        ));
    }

    let mut result = String::new();
    result.push_str(&String::from_utf8_lossy(&output.stdout));
    result.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(CallToolResult::success(vec![Content::text(result)]))
}
