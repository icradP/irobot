//获取当前日期时间
use crate::tools::ToolEntry;
use crate::tools::to_object;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
use chrono::Local;
use rmcp::{
    ErrorData,
    model::*,
    service::{RequestContext, RoleServer},
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use url::Url;

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "Get current datetime")]
pub struct GetCurrentDatetimeRequest {
    #[schemars(description = "City name to get local time for; empty = server local time")]
    #[serde(default)]
    pub city: Option<String>,
}
impl rmcp::service::ElicitationSafe for GetCurrentDatetimeRequest {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(GetCurrentDatetimeRequest);
    let tool = Tool {
        name: "get_current_datetime".into(),
        title: Some("Get Current Datetime".into()),
        description: Some("[Utility] Get the current datetime for a specific city or server local time".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "get_current_datetime",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    _context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let mut city: Option<String> = None;

    if let Some(args) = request {
        if let Ok(parsed) = serde_json::from_value::<GetCurrentDatetimeRequest>(args.clone()) {
            city = parsed.city;
        } else if let Some(m) = args.get("city").and_then(|v| v.as_str()) {
            city = Some(m.to_string());
        }
    }

    if let Some(c) = city {
        let c = c.trim();
        if !c.is_empty() {
            let default_key = "911cd2ec35d3467ab6c111928260701";
            let api_key = std::env::var("WEATHER_API_KEY").unwrap_or_else(|_| default_key.to_string());

            let mut url = Url::parse("http://api.weatherapi.com/v1/current.json")
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
            url.query_pairs_mut()
                .append_pair("key", &api_key)
                .append_pair("q", c)
                .append_pair("aqi", "no");

            let connector = HttpConnector::new();
            let client: Client<_, Full<Bytes>> = Client::builder(TokioExecutor::new()).build(connector);

            let req = Request::builder()
                .method(Method::GET)
                .uri(url.as_str())
                .body(Full::default())
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

            let res: hyper::Response<Incoming> = client
                .request(req)
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

            let status = res.status();
            let body_bytes = res
                .into_body()
                .collect()
                .await
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?
                .to_bytes();
            let raw: serde_json::Value = serde_json::from_slice(&body_bytes)
                .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;

            if !status.is_success() {
                return Err(ErrorData::internal_error(
                    format!("Weather API error: {}", raw),
                    None,
                ));
            }

            let localtime = raw["location"]["localtime"].as_str().unwrap_or("");
            if !localtime.is_empty() {
                return Ok(CallToolResult::success(vec![Content::text(localtime.to_string())]));
            }
        }
    }

    let now = Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    Ok(CallToolResult::success(vec![Content::text(now)]))
}

// registration is embedded in tool()
