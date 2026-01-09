use crate::tools::ToolEntry;
use crate::tools::to_object;
use rmcp::{
    ErrorData,
    model::*,
    service::{ElicitationError, RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

use tokio::process::Command;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "ffprobe 分析工具")]
pub struct FFProbeRequest {
    #[schemars(description = "输入文件路径或 URI（例如 file:///path/to/video.mp4）")]
    pub input: String,
}
impl rmcp::service::ElicitationSafe for FFProbeRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "ffprobe 引导参数")]
pub struct FFProbeElicitation {
    #[schemars(description = "输入文件路径或 URI（例如 file:///path/to/video.mp4）")]
    pub input: Option<String>,
}
impl rmcp::service::ElicitationSafe for FFProbeElicitation {}

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
    let mut input: Option<String> = None;
    let mut max_attempts: usize = 5;
    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            let m = m.clamp(1, 20) as usize;
            max_attempts = m;
        }
        if let Ok(parsed) = serde_json::from_value::<FFProbeRequest>(args.clone()) {
            input = Some(parsed.input);
        } else {
            input = args
                .get("input")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
        }
    }

    let mut prompt = "请输入要分析的媒体文件路径或 URI".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=ffprobe\nmessage=用户取消了本次工具调用",
            )]));
        }
        input = normalize_input(input);
        if let Some(ref s) = input {
            let cleaned = clean_path(s);
            let looks_like_remote_uri =
                cleaned.contains("://") && !cleaned.to_ascii_lowercase().starts_with("file://");
            if looks_like_remote_uri || Path::new(&cleaned).exists() {
                return analyze_media(&cleaned).await;
            }
            prompt = format!("路径不存在或不可访问，请重新输入文件路径或 URI：{}", cleaned);
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=ffprobe\nmessage=用户取消了本次工具调用",
                )]));
            }
            r = context.peer.elicit::<FFProbeElicitation>(prompt.clone()) => r,
        };
        match elicit_result {
            Ok(Some(params)) => {
                input = params.input;
                prompt = "请输入要分析的媒体文件路径或 URI".to_string();
            }
            Ok(None) => {
                prompt = "未获得输入，请重新输入媒体文件路径或 URI".to_string();
                input = None;
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=ffprobe\nmessage=用户取消了本次工具调用",
                )]))
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=ffprobe\nmessage=用户拒绝提供参数，本次工具调用已取消",
                )]))
            }
            Err(ElicitationError::ParseError { .. }) => {
                prompt = "输入格式不符合要求，请重新输入媒体文件路径或 URI".to_string();
                input = None;
            }
            Err(ElicitationError::NoContent) => {
                prompt = "未收到有效内容，请重新输入媒体文件路径或 URI".to_string();
                input = None;
            }
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
    }

    Ok(CallToolResult::success(vec![Content::text(format!(
        "tool_error\nname=ffprobe\nmessage=缺参引导已达到上限({})，仍未获得有效的媒体文件路径或 URI",
        max_attempts
    ))]))
}

fn normalize_input(input: Option<String>) -> Option<String> {
    let s = input?;
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    if t.eq_ignore_ascii_case("null") || t.eq_ignore_ascii_case("none") {
        return None;
    }
    Some(t.to_string())
}

async fn analyze_media(input: &str) -> Result<CallToolResult, ErrorData> {
    let output = match Command::new("ffprobe")
        .args(["-hide_banner", "-show_format", "-show_streams", input])
        .output()
        .await
    {
        Ok(o) => o,
        Err(e) => {
            let msg = format!(
                "tool_error\nname=ffprobe\ninput={}\nmessage={}",
                input,
                format!("Failed to execute ffprobe: {}", e)
            );
            return Ok(CallToolResult::success(vec![Content::text(msg)]));
        }
    };

    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr).to_string();
        let msg = format!(
            "tool_error\nname=ffprobe\ninput={}\nmessage={}",
            input,
            err.trim()
        );
        return Ok(CallToolResult::success(vec![Content::text(msg)]));
    }

    let mut result = String::new();
    result.push_str(&String::from_utf8_lossy(&output.stdout));
    result.push_str(&String::from_utf8_lossy(&output.stderr));

    Ok(CallToolResult::success(vec![Content::text(result)]))
}
