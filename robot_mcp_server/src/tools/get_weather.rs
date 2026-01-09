use crate::tools::ToolEntry;
use crate::tools::to_object;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::{Method, Request};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::TokioExecutor;
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
#[schemars(description = "City weather information request")]
pub struct GetWeatherRequest {
    #[schemars(description = "City name to get weather for")]
    pub city: String,
}
impl rmcp::service::ElicitationSafe for GetWeatherRequest {}

pub fn tool() -> ToolEntry {
    let schema = schemars::schema_for!(GetWeatherRequest);
    let tool = Tool {
        name: "get_weather".into(),
        title: Some("Get Weather".into()),
        description: Some("[Utility] Get current weather information for a specific city.".into()),
        input_schema: Arc::new(to_object(serde_json::to_value(schema).unwrap())),
        output_schema: None,
        annotations: None,
        icons: None,
        meta: None,
    };
    ToolEntry {
        name: "get_weather",
        tool,
        handler: Arc::new(|request, context, _state| Box::pin(handle(request, context))),
    }
}

pub async fn handle(
    request: Option<serde_json::Value>,
    context: RequestContext<RoleServer>,
) -> Result<CallToolResult, ErrorData> {
    let args: GetWeatherRequest = if let Some(args) = request {
        serde_json::from_value(args).map_err(|e| ErrorData::invalid_params(e.to_string(), None))?
    } else {
        // Request city parameter via elicitation when not provided
        match context
            .peer
            .elicit::<GetWeatherRequest>("请提供城市名称（例如：北京、上海、杭州）".to_string())
            .await
        {
            Ok(Some(params)) => params,
            Ok(None) => return Err(ErrorData::invalid_params("未提供城市名称", None)),
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
    };

    let city = args.city.trim();

    let default_key = "911cd2ec35d3467ab6c111928260701";
    let api_key = std::env::var("WEATHER_API_KEY").unwrap_or_else(|_| default_key.to_string());

    let mut url = Url::parse("http://api.weatherapi.com/v1/current.json")
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
    url.query_pairs_mut()
        .append_pair("key", &api_key)
        .append_pair("q", city)
        .append_pair("aqi", "yes");
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

    // Extract useful info
    let location = raw["location"]["name"].as_str().unwrap_or("Unknown");
    let region = raw["location"]["region"].as_str().unwrap_or("");
    let country = raw["location"]["country"].as_str().unwrap_or("");

    let current = &raw["current"];
    let temp_c = current["temp_c"].as_f64().unwrap_or(0.0);
    let condition = current["condition"]["text"].as_str().unwrap_or("Unknown");
    let feelslike_c = current["feelslike_c"].as_f64().unwrap_or(temp_c);
    let humidity = current["humidity"].as_u64().unwrap_or(0);
    let wind_kph = current["wind_kph"].as_f64().unwrap_or(0.0);
    let wind_dir = current["wind_dir"].as_str().unwrap_or("N");
    let uv = current["uv"].as_f64().unwrap_or(0.0);

    let pm2_5 = current["air_quality"]["pm2_5"].as_f64().unwrap_or(-1.0);
    let aqi_msg = if pm2_5 >= 0.0 {
        format!(", PM2.5: {:.1}", pm2_5)
    } else {
        "".to_string()
    };

    let text = format!(
        "Weather in {}, {}, {}: {}\nTemperature: {}°C (Feels like {}°C)\nWind: {} at {} km/h\nHumidity: {}%, UV: {}{}",
        location,
        region,
        country,
        condition,
        temp_c,
        feelslike_c,
        wind_dir,
        wind_kph,
        humidity,
        uv,
        aqi_msg
    );

    Ok(CallToolResult::success(vec![Content::text(text)]))
}

// registration is embedded in tool()
