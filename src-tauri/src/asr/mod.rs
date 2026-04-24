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
    pub whisper: std::sync::Mutex<Option<WhisperAsr>>,
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
