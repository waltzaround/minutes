//! Local LLM inference. `LocalLlm` is the seam: today it is backed by a
//! managed llama.cpp `llama-server` sidecar; embedded bindings can replace it
//! without touching callers.

pub mod llama;
pub mod model_manager;
pub mod prompts;
pub mod schemas;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRequest {
    pub system: String,
    pub user: String,
    pub schema: serde_json::Value,
    pub max_tokens: u32,
    pub temperature: f32,
}

#[derive(Debug, Clone, Default)]
pub struct JsonResponse {
    pub content: String,
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub prompt_tokens_per_second: f64,
    pub generation_tokens_per_second: f64,
    /// Generation stopped at `max_tokens` (output probably truncated).
    pub truncated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum LlmError {
    #[error("The summary engine is not installed with this app build.")]
    RuntimeMissing,
    #[error("The summary engine could not start: {0}")]
    StartFailed(String),
    #[error("The summary engine stopped unexpectedly.")]
    Crashed,
    #[error("The summary engine took too long to respond.")]
    Timeout,
    #[error("The summary engine returned an error: {0}")]
    Request(String),
}

/// Blocking interface; callers run it off the UI thread.
pub trait LocalLlm: Send + Sync {
    fn complete_json(&self, req: &JsonRequest) -> Result<JsonResponse, LlmError>;
    fn context_tokens(&self) -> u32;
    fn model_id(&self) -> &str;
}
