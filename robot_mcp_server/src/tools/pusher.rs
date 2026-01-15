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
use tokio::io::{AsyncBufReadExt, BufReader};
use std::process::Stdio;
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "推流工具 (JT1078)")]
pub struct PusherRequest {
    #[schemars(description = "输入 MP4 文件路径")]
    pub file_path: String,
    #[schemars(description = "目标 IP 地址")]
    pub ip: String,
    #[schemars(description = "目标端口号")]
    pub port: u16,
    #[schemars(description = "通道号")]
    pub channel: u16, //通道
    #[schemars(description = "设备 IMEI 号 (15位)")]
    pub imei: String,
}
impl rmcp::service::ElicitationSafe for PusherRequest {}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "推流工具引导参数")]
pub struct PusherElicitation {
    pub file_path: Option<String>,
    pub ip: Option<String>,
    pub port: Option<u16>,
    pub channel: Option<u16>, //通道
    pub imei: Option<String>, //设备 IMEI 号 (15位)
}
impl rmcp::service::ElicitationSafe for PusherElicitation {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(PusherRequest);
    let mut meta_map = serde_json::Map::new();
    meta_map.insert("isLongRunning".to_string(), serde_json::Value::Bool(true));
    
    let tool = Tool {
        name: "pusher_tool".into(),
        title: Some("JT1078 推流工具".into()),
        description: Some("使用 rtp_pusher 推送 MP4 文件到指定 IP 和端口 (JT1078协议)".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: Some(Meta(meta_map)),
    };
    ToolEntry {
        name: "pusher_tool",
        tool,
        handler: Arc::new(|req, ctx, _state| Box::pin(handle(req, ctx))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let mut file_path: Option<String> = None;
    let mut ip: Option<String> = None;
    let mut port: Option<u16> = None;
    let mut channel: Option<u16> = None;
    let mut imei: Option<String> = None;
    let mut is_ssl: Option<bool> = None;
    let mut cipher: Option<String> = None;
    
    let mut max_attempts: usize = 5;

    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = m.clamp(1, 20) as usize;
        }

        if let Ok(parsed) = serde_json::from_value::<PusherRequest>(args.clone()) {
            file_path = Some(parsed.file_path);
            ip = Some(parsed.ip);
            port = Some(parsed.port);
            imei = Some(parsed.imei);
            channel = Some(parsed.channel);
        } else {
            // Try to parse partial fields
            if let Some(v) = args.get("file_path").and_then(|v| v.as_str()) {
                file_path = Some(v.to_string());
            }
            if let Some(v) = args.get("ip").and_then(|v| v.as_str()) {
                ip = Some(v.to_string());
            }
            if let Some(v) = args.get("port").and_then(|v| v.as_u64()) {
                port = Some(v as u16);
            }
            if let Some(v) = args.get("channel").and_then(|v| v.as_u64()) {
                channel = Some(v as u16);
            }
            if let Some(v) = args.get("imei").and_then(|v| v.as_str()) {
                imei = Some(v.to_string());
            }
            if let Some(v) = args.get("is_ssl").and_then(|v| v.as_bool()) {
                is_ssl = Some(v);
            }
            if let Some(v) = args.get("cipher").and_then(|v| v.as_str()) {
                cipher = Some(v.to_string());
            }
        }
    }

    let mut prompt = "请输入推流参数".to_string();

    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=pusher\nmessage=用户取消了本次工具调用",
            )]));
        }

        // Validate and normalize
        let mut missing_params = Vec::new();
        
        if file_path.is_none() {
            missing_params.push("MP4 文件路径");
        }
        if ip.is_none() {
            missing_params.push("目标 IP");
        }
        if port.is_none() {
            missing_params.push("端口号");
        }
        if channel.is_none() {
            missing_params.push("通道号");
        }
        if imei.is_none() {
            missing_params.push("IMEI (15位)");
        }

        if missing_params.is_empty() {
            let f = file_path.as_ref().unwrap();
            let i = imei.as_ref().unwrap();
            
            // Basic validation
            if !f.ends_with(".mp4") {
                 prompt = format!("文件路径必须以 .mp4 结尾，当前: {}", f);
                 file_path = None; // Reset invalid
                 continue;
            }
            if !Path::new(f).exists() {
                 prompt = format!("文件不存在: {}", f);
                 file_path = None;
                 continue;
            }
            if channel.is_none() {
                prompt = format!("通道号不能为空");
                channel = None;
                continue;
            }
            if i.len() != 15 {
                 prompt = format!("IMEI 必须是 15 位数字，当前长度: {}", i.len());
                 imei = None;
                 continue;
            }
            

            // All good, run command
            return run_pusher(f, ip.as_ref().unwrap(), port.unwrap(), channel.unwrap(), i, context).await;
        }

        if prompt == "请输入推流参数" {
             prompt = format!("请输入: {}", missing_params.join(", "));
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=pusher\nmessage=用户取消了本次工具调用",
                )]));
            }
            r = context.peer.elicit::<PusherElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                if let Some(p) = params.file_path { file_path = Some(p); }
                if let Some(p) = params.ip { ip = Some(p); }
                if let Some(p) = params.port { port = Some(p); }
                if let Some(p) = params.channel { channel = Some(p); }
                if let Some(p) = params.imei { imei = Some(p); }
                prompt = format!("检查参数...缺少: {}", missing_params.join(", "));
            }
            Ok(None) => {
                 prompt = "未获得输入，请重新输入".to_string();
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=pusher\nmessage=用户取消了本次工具调用",
                )]))
            }
             Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=pusher\nmessage=用户拒绝提供参数，本次工具调用已取消",
                )]))
            }
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
    }

    Ok(CallToolResult::success(vec![Content::text(format!(
        "tool_error\nname=pusher\nmessage=缺参引导已达到上限({})，仍未获得有效参数",
        max_attempts
    ))]))
}

async fn run_pusher(
    file_path: &str,
    ip: &str,
    port: u16,
    channel: u16,
    imei: &str,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let tool_path = "/home/icrad/Rust/MCP/robot_mcp_server/3rdtools/rtp_pusher";

    let token = Uuid::new_v4().to_string();
    let token_val = NumberOrString::String(token.into());

    let mut child = match Command::new(tool_path)
        .arg(file_path)
        .arg(ip)
        .arg(port.to_string())
        .arg(channel.to_string())
        .arg(imei)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
             return Ok(CallToolResult::success(vec![Content::text(format!(
                "tool_error\nname=pusher\nmessage=执行失败: {}", e
            ))]));
        }
    };

    let _ = context.peer.notify_progress(ProgressNotificationParam {
        progress_token: ProgressToken(token_val.clone()),
        progress: 0.0,
        total: Some(100.0),
        message: Some("开始推流".into()),
    }).await;

    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    let mut progress: i32 = 0;

    if let Some(stdout) = child.stdout.take() {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stdout_buf.push_str(&line);
            stdout_buf.push('\n');

            if let Some(rest) = line.strip_prefix("progress ") {
                let value = rest.trim_end_matches('%').trim();
                if let Ok(v) = value.parse::<i32>() {
                    if v >= 0 && v <= 100 {
                        progress = v;
                        let _ = context.peer.notify_progress(ProgressNotificationParam {
                            progress_token: ProgressToken(token_val.clone()),
                            progress: v as f64,
                            total: Some(100.0),
                            message: Some(format!("推流进度 {}%", v)),
                        }).await;
                    }
                }
            }
        }
    }

    if let Some(stderr) = child.stderr.take() {
        let mut reader = BufReader::new(stderr).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            stderr_buf.push_str(&line);
            stderr_buf.push('\n');
        }
    }

    let status = match child.wait().await {
        Ok(s) => s,
        Err(e) => {
            return Ok(CallToolResult::success(vec![Content::text(format!(
                "tool_error\nname=pusher\nmessage=等待进程结束失败: {}", e
            ))]));
        }
    };

    if status.success() && progress < 100 {
        progress = 100;
        let _ = context.peer.notify_progress(ProgressNotificationParam {
            progress_token: ProgressToken(token_val.clone()),
            progress: 100.0,
            total: Some(100.0),
            message: Some("推流完成".into()),
        }).await;
    }

    let mut data = serde_json::Map::new();
    data.insert("progress".to_string(), serde_json::Value::Number(progress.into()));
    data.insert("stdout".to_string(), serde_json::Value::String(stdout_buf));
    data.insert("stderr".to_string(), serde_json::Value::String(stderr_buf));
    data.insert(
        "success".to_string(),
        serde_json::Value::Bool(status.success()),
    );

    let content = Content::json(data)?;
    Ok(CallToolResult::success(vec![content]))
}
