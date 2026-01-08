use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Persona {
    pub name: String,
    pub style: OutputStyle,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum OutputStyle {
    Neutral,
    Formal,
    Friendly,
}

impl Default for Persona {
    fn default() -> Self {
        Self {
            name: "RobotCore".to_string(),
            style: OutputStyle::Neutral,
        }
    }
}

