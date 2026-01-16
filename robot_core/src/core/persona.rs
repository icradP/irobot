use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    pub style: String,  
    pub nickname: Option<String>,
    pub background: Option<String>,
    pub preferences: Option<String>,
    pub banned_topics: Option<Vec<String>>,
    pub uuid: String
}

// 定义枚举
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OutputStyle {
    #[serde(rename = "neutral")]
    Neutral,
    #[serde(rename = "formal")]
    Formal,
    #[serde(rename = "friendly")]
    Friendly,
}

// 实现 Display（或者用 strum）
impl std::fmt::Display for OutputStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            OutputStyle::Neutral => "neutral",
            OutputStyle::Formal => "formal",
            OutputStyle::Friendly => "friendly",
        })
    }
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            name: "RobotCore".to_string(),
            style: OutputStyle::Neutral.to_string(),
            nickname: None,
            background: None,
            preferences: None,
            banned_topics: None,
            uuid: uuid::Uuid::new_v4().to_string(),
        }
    }
}
