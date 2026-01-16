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
    service::{ElicitationError, RequestContext, RoleServer},
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

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[schemars(description = "City weather information elicitation")]
pub struct GetWeatherElicitation {
    #[schemars(description = "City name to get weather for")]
    pub city: Option<String>,
}
impl rmcp::service::ElicitationSafe for GetWeatherElicitation {}

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
    let mut city: Option<String> = None;
    let mut max_attempts: usize = 5;

    if let Some(args) = request {
        if let Some(m) = args
            .get("__elicitation")
            .and_then(|v| v.get("max_attempts"))
            .and_then(|v| v.as_u64())
        {
            max_attempts = (m.clamp(1, 20)) as usize;
        }

        if let Ok(parsed) = serde_json::from_value::<GetWeatherRequest>(args.clone()) {
            city = Some(parsed.city);
        } else {
            if let Some(m) = args.get("city").and_then(|v| v.as_str()) {
                city = Some(m.to_string());
            }
        }
    }

    let mut prompt = "请提供城市名称（例如：北京、上海、杭州）".to_string();
    for _ in 0..max_attempts {
        if context.ct.is_cancelled() {
            return Ok(CallToolResult::success(vec![Content::text(
                "tool_cancel\nname=get_weather\nmessage=用户取消了天气查询请求",
            )]));
        }

        if city.is_some() {
            break;
        }

        let elicit_result = tokio::select! {
            _ = context.ct.cancelled() => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=get_weather\nmessage=用户取消了天气查询请求",
                )]));
            }
            r = context.peer.elicit::<GetWeatherElicitation>(prompt.clone()) => r,
        };

        match elicit_result {
            Ok(Some(params)) => {
                if let Some(m) = params.city {
                    if !m.trim().is_empty() {
                         city = Some(m);
                    }
                }
                if city.is_none() {
                    prompt = "仍缺少必要参数(city)，请补充：".to_string();
                }
            }
             Ok(None) => {
                prompt = "未获得有效城市名称，请重新提供".to_string();
            }
            Err(ElicitationError::UserCancelled) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=get_weather\nmessage=用户取消了天气查询请求",
                )]))
            }
            Err(ElicitationError::UserDeclined) => {
                return Ok(CallToolResult::success(vec![Content::text(
                    "tool_cancel\nname=get_weather\nmessage=用户拒绝提供参数，请求已取消",
                )]))
            }
            Err(ElicitationError::ParseError { .. }) => {
                prompt = "输入格式不符合要求，请重新提供".to_string();
            }
            Err(ElicitationError::NoContent) => {
                prompt = "未收到有效内容，请重新提供".to_string();
            }
            Err(e) => return Err(ErrorData::internal_error(format!("引导错误: {}", e), None)),
        }
        
        if city.is_some() {
            break;
        }
    }
    
    if city.is_none() {
         return Ok(CallToolResult::success(vec![Content::text(format!("tool_error\nname=get_weather\nmessage=缺参引导已达到上限({})，仍未获得有效的城市名称", max_attempts))]));
    }
    
    let city = city.unwrap();
    let city = city.trim();

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
    //可以用来查询对应的时间
    let localtime = raw["location"]["localtime"].as_str().unwrap_or("");

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
