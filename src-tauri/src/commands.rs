use crate::asr::{AsrState, WhisperAsr};
use crate::audio::AudioConfig;
use crate::db::{DbState, RewriteRecord};
use crate::inject::{InjectMode, TextInjector};
use crate::llm::{LlmState, ModelInfo, RewriteParams};
use crate::session::{DictationSession, SessionInfo, SessionStage, SessionState};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

const WHISPER_MODEL_PATH: &str = "research/phase0/whisper_models/ggml-small.bin";
const REQUIRED_MODELS: &[&str] = &["gemma4:e4b", "gemma4:e2b"];
const RING_BUFFER_SIZE: usize = 16000 * 60; // 60 seconds at 16kHz

#[tauri::command]
pub async fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
pub async fn list_models(state: State<'_, LlmState>) -> Result<Vec<ModelInfo>, String> {
    state
        .runtime
        .list_models()
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn rewrite_text(
    state: State<'_, LlmState>,
    model: String,
    prompt: String,
    max_new_tokens: Option<u32>,
) -> Result<String, String> {
    state
        .runtime
        .rewrite(RewriteParams {
            model,
            prompt,
            max_new_tokens: max_new_tokens.unwrap_or(512),
        })
        .await
        .map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn rewrite_streaming(
    app: AppHandle,
    state: State<'_, LlmState>,
    model: String,
    prompt: String,
    max_new_tokens: Option<u32>,
) -> Result<String, String> {
    let (tx, mut rx) = mpsc::channel::<String>(64);
    let runtime = state.runtime.clone();
    let params = RewriteParams {
        model,
        prompt,
        max_new_tokens: max_new_tokens.unwrap_or(512),
    };

    let app_clone = app.clone();
    tokio::spawn(async move {
        while let Some(token) = rx.recv().await {
            let _ = app_clone.emit("llm:token", &token);
        }
    });

    let result = runtime
        .rewrite_streaming(params, tx)
        .await
        .map_err(|e| format!("{e:#}"))?;

    let _ = app.emit("llm:done", &result);
    Ok(result)
}

#[tauri::command]
pub async fn get_session_info(
    state: State<'_, SessionState>,
) -> Result<Option<SessionInfo>, String> {
    let guard = state.current.lock().await;
    Ok(guard.as_ref().map(|s| s.info()))
}

#[tauri::command]
pub async fn grant_consent(state: State<'_, SessionState>) -> Result<(), String> {
    state
        .consent_given
        .store(true, std::sync::atomic::Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
pub async fn start_dictation(
    app: AppHandle,
    session_state: State<'_, SessionState>,
    asr_state: State<'_, AsrState>,
) -> Result<String, String> {
    if !session_state
        .consent_given
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err("Recording consent not granted".to_string());
    }

    let mut session_guard = session_state.current.lock().await;
    if session_guard.is_some() {
        return Err("Session already active".to_string());
    }

    // Load whisper model if not loaded
    let needs_load = {
        let guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
        guard.is_none()
    };
    if needs_load {
        let model_path = std::env::current_dir()
            .unwrap_or_default()
            .parent()
            .map(|p| p.join(WHISPER_MODEL_PATH))
            .unwrap_or_default();
        let path_str = model_path.to_string_lossy().to_string();
        let asr = tokio::task::spawn_blocking(move || WhisperAsr::new(&path_str))
            .await
            .map_err(|e| format!("join error: {e}"))?
            .map_err(|e| format!("whisper load failed: {e:#}"))?;
        let mut guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
        *guard = Some(asr);
    }

    // Start audio capture
    let (producer, consumer) = rtrb::RingBuffer::new(RING_BUFFER_SIZE);
    {
        let mut audio_guard = asr_state.audio.lock().map_err(|e| format!("{e}"))?;
        audio_guard
            .start(producer, AudioConfig::default())
            .map_err(|e| format!("audio start failed: {e:#}"))?;
    }
    {
        let mut consumer_guard = asr_state.ring_consumer.lock().map_err(|e| format!("{e}"))?;
        *consumer_guard = Some(consumer);
    }

    let session = DictationSession::new();
    let session_id = session.id.clone();
    *session_guard = Some(session);

    let _ = app.emit(
        "session:state",
        SessionInfo {
            id: session_id.clone(),
            stage: SessionStage::Recording,
        },
    );

    Ok(session_id)
}

#[tauri::command]
pub async fn stop_dictation(
    app: AppHandle,
    session_state: State<'_, SessionState>,
    asr_state: State<'_, AsrState>,
) -> Result<String, String> {
    // We'll clear the session at the end regardless of outcome
    struct ClearSession<'a>(&'a SessionState);
    impl Drop for ClearSession<'_> {
        fn drop(&mut self) {
            if let Ok(mut guard) = self.0.current.try_lock() {
                *guard = None;
            }
        }
    }
    let _clear = ClearSession(session_state.inner());

    // Stop audio capture
    {
        let mut audio_guard = asr_state.audio.lock().map_err(|e| format!("{e}"))?;
        audio_guard.stop();
    }

    // Small delay to let the audio thread flush remaining samples
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    // Drain ring buffer
    let samples: Vec<f32> = {
        let mut consumer_guard = asr_state.ring_consumer.lock().map_err(|e| format!("{e}"))?;
        match consumer_guard.take() {
            Some(mut consumer) => {
                let available = consumer.slots();
                log::info!("audio: {available} samples available in ring buffer");
                if available == 0 {
                    return Err("No audio captured".to_string());
                }
                let mut buf = Vec::with_capacity(available);
                let chunk = consumer.read_chunk(available).map_err(|e| format!("read_chunk: {e:?}"))?;
                let (first, second) = chunk.as_slices();
                buf.extend_from_slice(first);
                buf.extend_from_slice(second);
                chunk.commit_all();
                buf
            }
            None => return Err("No audio consumer available".to_string()),
        }
    };

    log::info!("audio: captured {} samples ({:.1}s at 16kHz)", samples.len(), samples.len() as f64 / 16000.0);

    let _ = app.emit("session:state", SessionInfo {
        id: String::new(),
        stage: SessionStage::Transcribing,
    });

    // Transcribe (CPU-blocking, must use spawn_blocking)
    let ctx_ptr = {
        let guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
        match guard.as_ref() {
            Some(whisper) => &whisper.ctx as *const _ as usize,
            None => return Err("Whisper model not loaded".to_string()),
        }
    };
    // guard is dropped here, safe to .await
    let transcript = tokio::task::spawn_blocking(move || {
        let ctx = unsafe { &*(ctx_ptr as *const whisper_rs::WhisperContext) };
        let mut params = whisper_rs::FullParams::new(
            whisper_rs::SamplingStrategy::Greedy { best_of: 1 },
        );
        params.set_language(Some("ja"));
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let mut state = ctx.create_state()
            .map_err(|e| format!("whisper state: {e}"))?;
        state.full(params, &samples)
            .map_err(|e| format!("whisper transcribe: {e}"))?;

        let n = state.full_n_segments();
        let mut text = String::new();
        for i in 0..n {
            if let Some(seg) = state.get_segment(i) {
                if let Ok(s) = seg.to_str() {
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(s.trim());
                }
            }
        }
        Ok::<String, String>(text)
    })
    .await
    .map_err(|e| format!("join: {e}"))?
    .map_err(|e| format!("transcribe: {e}"))?;

    log::info!("asr: transcript = {:?}", &transcript);
    let _ = app.emit("asr:final", &transcript);

    Ok(transcript)
}

#[tauri::command]
pub async fn inject_text(text: String, mode: Option<String>) -> Result<(), String> {
    let inject_mode = match mode.as_deref() {
        Some("clipboard") => InjectMode::Clipboard,
        _ => InjectMode::Direct,
    };
    TextInjector::inject(&text, inject_mode).map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn search_history(
    state: State<'_, DbState>,
    query: String,
    limit: Option<usize>,
) -> Result<Vec<RewriteRecord>, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db
            .search_rewrites(&query, limit.unwrap_or(50))
            .map_err(|e| format!("{e:#}")),
        None => Err("Database not initialized".to_string()),
    }
}

#[tauri::command]
pub async fn list_history(
    state: State<'_, DbState>,
    limit: Option<usize>,
) -> Result<Vec<RewriteRecord>, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db
            .list_rewrites(limit.unwrap_or(50))
            .map_err(|e| format!("{e:#}")),
        None => Err("Database not initialized".to_string()),
    }
}

// --- Setup / Onboarding ---

#[derive(Debug, Clone, Serialize)]
pub struct SetupStatus {
    pub ollama_running: bool,
    pub ollama_version: Option<String>,
    pub models_installed: Vec<String>,
    pub models_missing: Vec<String>,
    pub whisper_available: bool,
    pub ready: bool,
}

#[tauri::command]
pub async fn check_setup(state: State<'_, LlmState>) -> Result<SetupStatus, String> {
    let mut status = SetupStatus {
        ollama_running: false,
        ollama_version: None,
        models_installed: vec![],
        models_missing: vec![],
        whisper_available: false,
        ready: false,
    };

    // Check Ollama
    match reqwest::get("http://127.0.0.1:11434/api/version").await {
        Ok(resp) => {
            status.ollama_running = true;
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                status.ollama_version = body["version"].as_str().map(|s| s.to_string());
            }
        }
        Err(_) => return Ok(status),
    }

    // Check models
    match state.runtime.list_models().await {
        Ok(models) => {
            let installed: Vec<String> = models.iter().map(|m| m.name.clone()).collect();
            for &required in REQUIRED_MODELS {
                if installed.iter().any(|n| n == required) {
                    status.models_installed.push(required.to_string());
                } else {
                    status.models_missing.push(required.to_string());
                }
            }
        }
        Err(_) => {
            status.models_missing = REQUIRED_MODELS.iter().map(|s| s.to_string()).collect();
        }
    }

    // Check whisper model
    let whisper_path = std::env::current_dir()
        .unwrap_or_default()
        .parent()
        .map(|p| p.join(WHISPER_MODEL_PATH))
        .unwrap_or_default();
    status.whisper_available = whisper_path.exists();

    status.ready = status.ollama_running
        && status.models_missing.is_empty()
        && status.whisper_available;

    Ok(status)
}

#[tauri::command]
pub async fn pull_model(app: AppHandle, model: String) -> Result<(), String> {
    let payload = serde_json::json!({ "name": model, "stream": true });
    let client = reqwest::Client::new();
    let resp = client
        .post("http://127.0.0.1:11434/api/pull")
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("pull request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("ollama pull returned {}", resp.status()));
    }

    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buffer = String::new();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream error: {e}"))?;
        buffer.push_str(&String::from_utf8_lossy(&chunk));

        while let Some(pos) = buffer.find('\n') {
            let line = buffer[..pos].trim().to_string();
            buffer = buffer[pos + 1..].to_string();
            if line.is_empty() {
                continue;
            }
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                let status = val["status"].as_str().unwrap_or("").to_string();
                let total = val["total"].as_u64().unwrap_or(0);
                let completed = val["completed"].as_u64().unwrap_or(0);
                let _ = app.emit("model:pull:progress", serde_json::json!({
                    "model": &model,
                    "status": status,
                    "total": total,
                    "completed": completed,
                }));
            }
        }
    }

    let _ = app.emit("model:pull:done", &model);
    Ok(())
}
