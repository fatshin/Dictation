use anyhow::Result;
use serde::Serialize;

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

pub struct WhisperAsr {
    pub ctx: whisper_rs::WhisperContext,
}

unsafe impl Send for WhisperAsr {}
unsafe impl Sync for WhisperAsr {}

impl WhisperAsr {
    pub fn new(model_path: &str) -> Result<Self> {
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow::anyhow!("failed to load whisper model: {e}"))?;
        Ok(Self { ctx })
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

pub fn resolve_whisper_model_path(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    use tauri::Manager;

    if let Ok(env_path) = std::env::var("DICTATION_WHISPER_MODEL") {
        let p = std::path::PathBuf::from(env_path);
        if p.exists() {
            return Some(p);
        }
    }

    // Bundled resource (shipped inside the installer)
    if let Ok(resource) = app.path().resource_dir() {
        let p = resource.join("models").join("ggml-small.bin");
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(local) = app.path().app_local_data_dir() {
        let p = local.join("models").join("ggml-small.bin");
        if p.exists() {
            return Some(p);
        }
    }

    let dev_fallback = std::env::current_dir()
        .ok()
        .and_then(|d| d.parent().map(|p| p.to_path_buf()))
        .map(|p| p.join("research/phase0/whisper_models/ggml-small.bin"));
    if let Some(p) = dev_fallback {
        if p.exists() {
            return Some(p);
        }
    }

    None
}
