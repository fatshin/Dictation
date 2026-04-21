//! LLM runtime abstraction (ADR-002: Ollama HTTP loopback).
//!
//! Phase-1a contract:
//!   - `LlmRuntime::list_models()` enumerates locally-available models.
//!   - `LlmRuntime::rewrite()` runs a single completion against a model+prompt
//!     and returns the rewritten text (non-streaming for the scaffold; the
//!     streaming path lands when the overlay window does).
//!
//! Default impl talks to `127.0.0.1:11434/api/generate` with `think:false`
//! per the Phase-0 Day-5 finding: Gemma-4 default Thinking-Mode hides
//! response when streamed via Ollama. Phase-0 results
//! (`research/phase0/results/report.md`) drove the Tier-1 selection of
//! `gemma4:e4b` (primary) + `gemma4:e2b` (latency fallback).

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

pub const DEFAULT_OLLAMA_HOST: &str = "http://127.0.0.1:11434";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub size_bytes: u64,
    pub family: Option<String>,
    pub quantization: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewriteParams {
    pub model: String,
    pub prompt: String,
    /// Cap on generated tokens. Phase-0 default is 512; UI can lower.
    pub max_new_tokens: u32,
}

#[async_trait::async_trait]
pub trait LlmRuntime: Send + Sync {
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    async fn rewrite(&self, params: RewriteParams) -> Result<String>;
}

// ---------------------------------------------------------------------------
// Ollama HTTP impl
// ---------------------------------------------------------------------------

pub struct OllamaRuntime {
    base: String,
    http: reqwest::Client,
}

impl OllamaRuntime {
    pub fn new(base: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .build()
            .expect("reqwest client");
        Self {
            base: base.into(),
            http,
        }
    }
}

impl Default for OllamaRuntime {
    fn default() -> Self {
        Self::new(DEFAULT_OLLAMA_HOST)
    }
}

#[derive(Deserialize)]
struct OllamaListResponse {
    models: Vec<OllamaListModel>,
}

#[derive(Deserialize)]
struct OllamaListModel {
    name: String,
    size: u64,
    details: Option<OllamaListDetails>,
}

#[derive(Deserialize)]
struct OllamaListDetails {
    family: Option<String>,
    quantization_level: Option<String>,
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
    think: bool,
    options: GenerateOptions,
}

#[derive(Serialize)]
struct GenerateOptions {
    temperature: f32,
    num_predict: u32,
}

#[derive(Deserialize)]
struct GenerateResponse {
    response: String,
}

#[async_trait::async_trait]
impl LlmRuntime for OllamaRuntime {
    async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        let url = format!("{}/api/tags", self.base);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            return Err(anyhow!("ollama /api/tags returned {}", resp.status()));
        }
        let parsed: OllamaListResponse = resp.json().await.context("decode /api/tags")?;
        Ok(parsed
            .models
            .into_iter()
            .map(|m| ModelInfo {
                name: m.name,
                size_bytes: m.size,
                family: m.details.as_ref().and_then(|d| d.family.clone()),
                quantization: m.details.and_then(|d| d.quantization_level),
            })
            .collect())
    }

    async fn rewrite(&self, params: RewriteParams) -> Result<String> {
        let url = format!("{}/api/generate", self.base);
        let body = GenerateRequest {
            model: &params.model,
            prompt: &params.prompt,
            stream: false,
            // Day-5 finding: default Thinking-Mode hides the response field.
            // Until streaming UI lands, force think=false for non-stream path.
            think: false,
            options: GenerateOptions {
                temperature: 0.0,
                num_predict: params.max_new_tokens,
            },
        };
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {url}"))?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("ollama /api/generate returned {status}: {text}"));
        }
        let parsed: GenerateResponse = resp.json().await.context("decode /api/generate")?;
        Ok(parsed.response)
    }
}
