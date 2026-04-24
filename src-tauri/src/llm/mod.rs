use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

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
    pub max_new_tokens: u32,
}

#[async_trait::async_trait]
pub trait LlmRuntime: Send + Sync {
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    async fn rewrite(&self, params: RewriteParams) -> Result<String>;
    async fn rewrite_streaming(
        &self,
        params: RewriteParams,
        tx: mpsc::Sender<String>,
    ) -> Result<String>;
}

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
    #[serde(default)]
    done: bool,
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

    async fn rewrite_streaming(
        &self,
        params: RewriteParams,
        tx: mpsc::Sender<String>,
    ) -> Result<String> {
        let url = format!("{}/api/generate", self.base);
        let body = GenerateRequest {
            model: &params.model,
            prompt: &params.prompt,
            stream: true,
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

        let mut full_response = String::new();
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("stream read error")?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(newline_pos) = buffer.find('\n') {
                let line = buffer[..newline_pos].trim().to_string();
                buffer = buffer[newline_pos + 1..].to_string();

                if line.is_empty() {
                    continue;
                }

                if let Ok(parsed) = serde_json::from_str::<GenerateResponse>(&line) {
                    if !parsed.response.is_empty() {
                        full_response.push_str(&parsed.response);
                        let _ = tx.send(parsed.response).await;
                    }
                    if parsed.done {
                        return Ok(full_response);
                    }
                }
            }
        }

        Ok(full_response)
    }
}

pub struct LlmState {
    pub runtime: std::sync::Arc<dyn LlmRuntime>,
}

impl LlmState {
    pub fn new() -> Self {
        Self {
            runtime: std::sync::Arc::new(OllamaRuntime::default()),
        }
    }
}
