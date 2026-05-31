use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AsrResult {
    pub segments: Vec<Segment>,
    pub full_text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperModelInfo {
    pub id: &'static str,
    pub filename: &'static str,
    pub display_name: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    pub is_bundled: bool,
}

pub static WHISPER_MODELS: &[WhisperModelInfo] = &[
    WhisperModelInfo {
        id: "small",
        filename: "ggml-small.bin",
        display_name: "Small (466 MB)",
        size_bytes: 466_000_000,
        sha256: "",
        is_bundled: true,
    },
    WhisperModelInfo {
        id: "medium",
        filename: "ggml-medium.bin",
        display_name: "Medium (1.5 GB) — より高精度",
        size_bytes: 1_530_000_000,
        sha256: "",
        is_bundled: false,
    },
    WhisperModelInfo {
        id: "large-v3-turbo",
        filename: "ggml-large-v3-turbo.bin",
        display_name: "Large v3 Turbo (1.6 GB) — 最高精度",
        size_bytes: 1_620_000_000,
        sha256: "",
        is_bundled: false,
    },
];

pub struct WhisperAsr {
    pub ctx: whisper_rs::WhisperContext,
    pub model_id: String,
}

unsafe impl Send for WhisperAsr {}
unsafe impl Sync for WhisperAsr {}

impl WhisperAsr {
    pub fn new(model_path: &str, model_id: &str) -> Result<Self> {
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow::anyhow!("failed to load whisper model: {e}"))?;
        Ok(Self {
            ctx,
            model_id: model_id.to_string(),
        })
    }
}

pub struct AsrState {
    pub whisper: std::sync::Mutex<Option<std::sync::Arc<WhisperAsr>>>,
    pub audio: std::sync::Mutex<crate::audio::AudioCapture>,
    pub ring_consumer: std::sync::Mutex<Option<rtrb::Consumer<f32>>>,
}

impl AsrState {
    pub fn new() -> Self {
        Self {
            whisper: std::sync::Mutex::new(None),
            audio: std::sync::Mutex::new(crate::audio::AudioCapture::new()),
            ring_consumer: std::sync::Mutex::new(None),
        }
    }
}

impl Default for AsrState {
    fn default() -> Self {
        Self::new()
    }
}

pub fn resolve_whisper_model_path(
    app: &tauri::AppHandle,
    model_id: &str,
) -> Option<std::path::PathBuf> {
    use tauri::Manager;

    let filename = WHISPER_MODELS
        .iter()
        .find(|m| m.id == model_id)
        .map(|m| m.filename)
        .unwrap_or("ggml-small.bin");

    if let Ok(env_path) = std::env::var("DICTATION_WHISPER_MODEL") {
        let p = std::path::PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(resource) = app.path().resource_dir() {
        let p = resource.join("models").join(filename);
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(local) = app.path().app_local_data_dir() {
        let p = local.join("models").join(filename);
        if p.exists() {
            return Some(p);
        }
    }

    let dev_fallback = std::env::current_dir()
        .ok()
        .and_then(|d| d.parent().map(|p| p.to_path_buf()))
        .map(|p| p.join("research/phase0/whisper_models").join(filename));
    if let Some(p) = dev_fallback {
        if p.exists() {
            return Some(p);
        }
    }

    None
}
