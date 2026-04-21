//! Tauri command surface (Rust ↔ React).
//!
//! Phase-1a slice: enough to wire a "select model + paste text + see
//! rewritten output" smoke loop in the UI. Streaming, hotkey, ASR, history
//! all land in subsequent slices.

use crate::llm::{LlmRuntime, ModelInfo, OllamaRuntime, RewriteParams};
use std::sync::Arc;
use tauri::State;

pub struct AppState {
    pub llm: Arc<dyn LlmRuntime>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            llm: Arc::new(OllamaRuntime::default()),
        }
    }
}

#[tauri::command]
pub async fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
pub async fn list_models(state: State<'_, AppState>) -> Result<Vec<ModelInfo>, String> {
    state
        .llm
        .list_models()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn rewrite_text(
    state: State<'_, AppState>,
    model: String,
    prompt: String,
    max_new_tokens: Option<u32>,
) -> Result<String, String> {
    state
        .llm
        .rewrite(RewriteParams {
            model,
            prompt,
            max_new_tokens: max_new_tokens.unwrap_or(512),
        })
        .await
        .map_err(|e| format!("{e:#}"))
}
