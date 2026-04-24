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
    ctx: whisper_rs::WhisperContext,
}

impl WhisperAsr {
    pub fn new(model_path: &str) -> Result<Self> {
        let ctx = whisper_rs::WhisperContext::new_with_params(
            model_path,
            whisper_rs::WhisperContextParameters::default(),
        )
        .map_err(|e| anyhow::anyhow!("failed to load whisper model: {e}"))?;
        Ok(Self { ctx })
    }

    pub fn transcribe(&self, samples: &[f32], language: Option<&str>) -> Result<AsrResult> {
        let mut params = whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        if let Some(lang) = language {
            params.set_language(Some(lang));
        }
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let mut state = self.ctx.create_state()
            .map_err(|e| anyhow::anyhow!("failed to create whisper state: {e}"))?;

        state.full(params, samples)
            .map_err(|e| anyhow::anyhow!("whisper transcription failed: {e}"))?;

        let n = state.full_n_segments();

        let mut segments = Vec::new();
        let mut full_text = String::new();

        for i in 0..n {
            let seg = state.get_segment(i)
                .ok_or_else(|| anyhow::anyhow!("failed to get segment {i}"))?;
            let text = seg.to_str()
                .map_err(|e| anyhow::anyhow!("failed to get segment text: {e}"))?;
            let start = seg.start_timestamp();
            let end = seg.end_timestamp();

            if !full_text.is_empty() {
                full_text.push(' ');
            }
            full_text.push_str(text.trim());

            segments.push(Segment {
                start_ms: (start * 10) as u64,
                end_ms: (end * 10) as u64,
                text: text.trim().to_string(),
            });
        }

        Ok(AsrResult {
            segments,
            full_text,
        })
    }
}

pub struct AsrState {
    pub whisper: tokio::sync::Mutex<Option<WhisperAsr>>,
}

impl AsrState {
    pub fn new() -> Self {
        Self {
            whisper: tokio::sync::Mutex::new(None),
        }
    }
}
