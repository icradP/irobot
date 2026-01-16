use crate::core::perception::PerceptionData;
use crate::core::persona::Persona;
use crate::llm::adapter::{ChatMessage, ChatRequest, LLMClient};
use async_trait::async_trait;
use tracing::info;

#[derive(Debug, Clone, PartialEq)]
pub enum IntentDecision {
    Act,      // Proceed to DecisionEngine
    Ignore,   // Do nothing
}

#[async_trait]
pub trait IntentModule: Send + Sync {
    async fn evaluate(
        &self,
        persona: &Persona,
        perception: &PerceptionData,
        input_text: &str,
    ) -> anyhow::Result<IntentDecision>;
}

pub struct LLMIntentModule {
    pub llm: Box<dyn LLMClient + Send + Sync>,
    pub model: String,
}

impl LLMIntentModule {
    pub fn new(llm: Box<dyn LLMClient + Send + Sync>, model: String) -> Self {
        Self { llm, model }
    }
}

#[async_trait]
impl IntentModule for LLMIntentModule {
    async fn evaluate(
        &self,
        persona: &Persona,
        perception: &PerceptionData,
        input_text: &str,
    ) -> anyhow::Result<IntentDecision> {
        // The "Soul Question": Should I respond?
        let system_prompt = format!(
            "You named '{}' with a {:?} style.\n\
            \n\
            Perception of input:\n\
            Sentiment: {}\n\
            Urgency: {}\n\
            Context: {}\n\
            \n\
            You are receiving a message. Your task is to decide whether to RESPOND or IGNORE.\n\
            \n\
            Guidelines:\n\
            1. If the message is a direct question, a command, or explicitly addressed to you, RESPOND.\n\
            2. If the message is ambiguous but likely requires an answer (e.g., 'How is the weather?'), RESPOND.\n\
            3. If the message is just noise, irrelevant, or clearly addressed to someone else, IGNORE.\n\
            \n\
            Format your answer exactly like this:\n\
            Reason: [Short explanation of why]\n\
            Decision: [RESPOND or IGNORE]",
            persona.name,
            persona.style,
            perception.sentiment,
            perception.urgency,
            perception.context_summary
        );

        let user_prompt = format!("Message: {}", input_text);

        let req = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".into(),
                    content: system_prompt,
                },
                ChatMessage {
                    role: "user".into(),
                    content: user_prompt,
                },
            ],
            temperature: Some(0.1), // Deterministic
            session_id: None, // Intent analysis is internal, usually no need to stream think?
        };

        let out = self.llm.chat(req).await?;
        let output_text = out.text.trim();
        
        info!("Intent analysis:\n{}", output_text);

        let decision_str = output_text.to_uppercase();
        if decision_str.contains("DECISION: RESPOND") || decision_str.contains("DECISION:RESPOND") {
            Ok(IntentDecision::Act)
        } else {
            Ok(IntentDecision::Ignore)
        }
    }
}

pub struct BasicIntentModule;

#[async_trait]
impl IntentModule for BasicIntentModule {
    async fn evaluate(
        &self,
        _persona: &Persona,
        _perception: &PerceptionData,
        _input_text: &str,
    ) -> anyhow::Result<IntentDecision> {
        // Default to always responding if no LLM is used
        Ok(IntentDecision::Act)
    }
}
