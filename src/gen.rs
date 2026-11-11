//! Messages a running generation sends back to the UI.

use crate::jv::*;
use serde_json::Value;

pub enum GenEvent {
    Chunk(String),
    /// Token counts the backend reported for this request.
    Usage(crate::conn::Usage),
    Done,
    Error(String),
}

/// `(generation id, event)` so stale events from an aborted request can be ignored.
pub type GenMsg = (u64, GenEvent);

/// Which settings key holds the model name for each chat-completion source.
pub fn model_for(o: &Value, source: &str) -> String {
    let key = match source {
        "openai" => "openai_model",
        "claude" => "claude_model",
        "openrouter" => "openrouter_model",
        "ai21" => "ai21_model",
        "makersuite" => "google_model",
        "vertexai" => "vertexai_model",
        "mistralai" => "mistralai_model",
        "custom" => "custom_model",
        "cohere" => "cohere_model",
        "perplexity" => "perplexity_model",
        "groq" => "groq_model",
        "electronhub" => "electronhub_model",
        "chutes" => "chutes_model",
        "nanogpt" => "nanogpt_model",
        "deepseek" => "deepseek_model",
        "aimlapi" => "aimlapi_model",
        "xai" => "xai_model",
        "pollinations" => "pollinations_model",
        "moonshot" => "moonshot_model",
        "fireworks" => "fireworks_model",
        "cometapi" => "cometapi_model",
        "azure_openai" => "azure_openai_model",
        "zai" => "zai_model",
        "siliconflow" => "siliconflow_model",
        "workers_ai" => "workers_ai_model",
        "minimax" => "minimax_model",
        _ => "",
    };
    if key.is_empty() {
        String::new()
    } else {
        jstr(o, key)
    }
}
