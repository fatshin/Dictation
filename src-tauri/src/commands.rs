use crate::asr::{resolve_whisper_model_path, AsrState, WhisperAsr, WHISPER_MODELS};
use crate::audio::AudioConfig;
use crate::db::{
    AppSettings, DbState, DictionaryEntry, DictionaryUpsert, PromptTemplate, PromptTemplateUpsert,
    RewriteRecord, BUILTIN_PROMPTS,
};
use crate::inject::{
    get_focused_field_context, is_ax_trusted, is_external_focused, FocusedFieldContext, InjectMode,
    TextInjector,
};
use crate::llm::{LlmState, ModelInfo, RewriteParams};
use crate::session::{DictationSession, SessionInfo, SessionStage, SessionState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::mpsc;

// Default candidate set. The wizard considers setup "ready" if at least
// ONE of these is installed (not all of them) — see check_setup. Order
// reflects preference for 16GB-RAM CPU daily-driver use:
//   1. qwen3.5:4b-q4_K_M           — primary, balanced quality/speed
//   2. qwen3:4b-instruct-2507-q4_K_M — gen-over-gen A/B reference
//   3. llm-jp-3-3.7b (HF tag)      — JP-specialised alternate
//   4. gemma4:e4b / e2b            — pre-existing fallbacks
//
// qwen3.5:9b is intentionally NOT here — too slow for daily use on 16GB
// CPU; bench only. See research/phase0/ollama_candidates.json.
const REQUIRED_MODELS: &[&str] = &[
    "qwen3.5:4b-q4_K_M",
    "qwen3:4b-instruct-2507-q4_K_M",
    "hf.co/alfredplpl/llm-jp-3-3.7b-instruct-gguf:Q4_K_M",
    "gemma4:e4b",
    "gemma4:e2b",
];
const RING_BUFFER_SIZE: usize = 16000 * 60; // 60 seconds at 16kHz

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct JsonFallbackStore {
    #[serde(default)]
    settings: AppSettings,
    #[serde(default)]
    dictionary: Vec<DictionaryEntry>,
    #[serde(default)]
    prompts: Vec<PromptTemplate>,
}

fn json_store_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    let local_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("{e}"))?;
    std::fs::create_dir_all(&local_dir).map_err(|e| format!("create app data dir: {e}"))?;
    Ok(local_dir.join("dictation.json"))
}

fn load_json_store(app: &AppHandle) -> Result<JsonFallbackStore, String> {
    let path = json_store_path(app)?;
    match std::fs::read_to_string(&path) {
        Ok(raw) => match serde_json::from_str(&raw) {
            Ok(store) => Ok(store),
            Err(e) => {
                log::warn!("json fallback store is unreadable; using defaults: {e}");
                Ok(JsonFallbackStore::default())
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(JsonFallbackStore::default()),
        Err(e) => Err(format!("read json fallback store: {e}")),
    }
}

pub(crate) fn load_fallback_app_settings(app: &AppHandle) -> Result<AppSettings, String> {
    Ok(load_json_store(app)?.settings)
}

fn save_json_store(app: &AppHandle, store: &JsonFallbackStore) -> Result<(), String> {
    let path = json_store_path(app)?;
    let tmp = path.with_extension("json.tmp");
    let body =
        serde_json::to_string_pretty(store).map_err(|e| format!("encode json store: {e}"))?;
    std::fs::write(&tmp, body).map_err(|e| format!("write json fallback store: {e}"))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("replace json fallback store: {e}"))
}

fn fallback_timestamp() -> String {
    "1970-01-01 00:00:00".to_string()
}

fn now_timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.to_string()
}

fn builtin_prompt_templates() -> Vec<PromptTemplate> {
    let ts = fallback_timestamp();
    BUILTIN_PROMPTS
        .iter()
        .enumerate()
        .map(|(idx, (name, label, body, language))| PromptTemplate {
            id: format!("builtin_{name}"),
            name: (*name).to_string(),
            label: (*label).to_string(),
            body: (*body).to_string(),
            language: (*language).to_string(),
            is_builtin: true,
            order_idx: idx as i32,
            created_at: ts.clone(),
            updated_at: ts.clone(),
        })
        .collect()
}

fn list_json_prompts(store: &JsonFallbackStore) -> Vec<PromptTemplate> {
    let mut by_id: HashMap<String, PromptTemplate> = builtin_prompt_templates()
        .into_iter()
        .map(|p| (p.id.clone(), p))
        .collect();
    for prompt in &store.prompts {
        if let Some(base) = by_id.get(&prompt.id).cloned() {
            let mut merged = prompt.clone();
            if base.is_builtin {
                merged.is_builtin = true;
                merged.order_idx = base.order_idx;
                merged.created_at = base.created_at;
            }
            by_id.insert(prompt.id.clone(), merged);
        } else {
            by_id.insert(prompt.id.clone(), prompt.clone());
        }
    }
    let mut prompts: Vec<PromptTemplate> = by_id.into_values().collect();
    prompts.sort_by(|a, b| {
        a.order_idx
            .cmp(&b.order_idx)
            .then_with(|| a.name.cmp(&b.name))
    });
    prompts
}

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
            keep_alive: None,
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
        keep_alive: None,
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
pub fn list_audio_devices() -> Result<Vec<crate::audio::AudioDeviceInfo>, String> {
    crate::audio::list_input_devices().map_err(|e| format!("{e:#}"))
}

#[derive(Serialize)]
pub struct WhisperModelStatus {
    pub id: String,
    pub display_name: String,
    pub size_bytes: u64,
    pub is_bundled: bool,
    pub available: bool,
}

#[tauri::command]
pub fn list_whisper_models(app: AppHandle) -> Vec<WhisperModelStatus> {
    WHISPER_MODELS
        .iter()
        .map(|m| WhisperModelStatus {
            id: m.id.to_string(),
            display_name: m.display_name.to_string(),
            size_bytes: m.size_bytes,
            is_bundled: m.is_bundled,
            available: resolve_whisper_model_path(&app, m.id).is_some(),
        })
        .collect()
}

#[tauri::command]
pub async fn download_whisper_model(app: AppHandle, model_id: String) -> Result<(), String> {
    use futures_util::StreamExt;
    use sha2::{Digest, Sha256};

    let model_info = WHISPER_MODELS
        .iter()
        .find(|m| m.id == model_id)
        .ok_or_else(|| format!("Unknown model: {model_id}"))?;

    let local_dir = app
        .path()
        .app_local_data_dir()
        .map_err(|e| format!("{e}"))?;
    let models_dir = local_dir.join("models");
    std::fs::create_dir_all(&models_dir).map_err(|e| format!("create models dir: {e}"))?;
    let dest = models_dir.join(model_info.filename);

    let download_url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
        model_info.filename
    );

    let client = reqwest::Client::new();
    let resp = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("download failed: HTTP {}", resp.status()));
    }

    let total = resp.content_length().unwrap_or(model_info.size_bytes);
    let mut stream = resp.bytes_stream();
    let tmp_dest = dest.with_extension("bin.tmp");
    let mut file = std::fs::File::create(&tmp_dest).map_err(|e| format!("create file: {e}"))?;
    let mut downloaded: u64 = 0;
    let mut hasher = Sha256::new();

    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("stream: {e}"))?;
        file.write_all(&chunk).map_err(|e| format!("write: {e}"))?;
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        let _ = app.emit(
            "whisper:download:progress",
            serde_json::json!({
                "model_id": model_id,
                "downloaded": downloaded,
                "total": total,
            }),
        );
    }
    file.flush().map_err(|e| format!("flush: {e}"))?;
    drop(file);

    if !model_info.sha256.is_empty() {
        let hash = format!("{:x}", hasher.finalize());
        if hash != model_info.sha256 {
            let _ = std::fs::remove_file(&tmp_dest);
            return Err(format!(
                "SHA-256 mismatch: expected {}, got {hash}",
                model_info.sha256
            ));
        }
    }

    std::fs::rename(&tmp_dest, &dest).map_err(|e| format!("rename: {e}"))?;

    let _ = app.emit("whisper:download:done", &model_id);
    Ok(())
}

#[tauri::command]
pub async fn start_dictation(
    app: AppHandle,
    session_state: State<'_, SessionState>,
    asr_state: State<'_, AsrState>,
    db_state: State<'_, DbState>,
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

    let (device_name, whisper_model_id) = {
        let guard = db_state.db.lock().await;
        let settings = guard
            .as_ref()
            .and_then(|db| db.get_app_settings().ok())
            .unwrap_or_else(|| {
                load_json_store(&app)
                    .map(|store| store.settings)
                    .unwrap_or_default()
            });
        (settings.input_device, settings.whisper_model)
    };

    let needs_load = {
        let guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
        match guard.as_ref() {
            None => true,
            Some(asr) => asr.model_id != whisper_model_id,
        }
    };
    if needs_load {
        {
            let mut guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
            *guard = None;
        }
        let model_path = resolve_whisper_model_path(&app, &whisper_model_id).ok_or_else(|| {
            format!(
                "Whisper model '{}' not found. Download it from Settings.",
                whisper_model_id
            )
        })?;
        let path_str = model_path.to_string_lossy().to_string();
        let mid = whisper_model_id.clone();
        let asr = tokio::task::spawn_blocking(move || WhisperAsr::new(&path_str, &mid))
            .await
            .map_err(|e| format!("join error: {e}"))?
            .map_err(|e| format!("whisper load failed: {e:#}"))?;
        let mut guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
        *guard = Some(Arc::new(asr));
    }

    // Start audio capture
    let (producer, consumer) = rtrb::RingBuffer::new(RING_BUFFER_SIZE);
    {
        let mut audio_guard = asr_state.audio.lock().map_err(|e| format!("{e}"))?;
        audio_guard
            .start(
                producer,
                AudioConfig {
                    device_name,
                    ..AudioConfig::default()
                },
            )
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
    db_state: State<'_, DbState>,
) -> Result<String, String> {
    {
        let mut session_guard = session_state.current.lock().await;
        if session_guard.take().is_none() {
            return Ok(String::new());
        }
    }

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
                let chunk = consumer
                    .read_chunk(available)
                    .map_err(|e| format!("read_chunk: {e:?}"))?;
                let (first, second) = chunk.as_slices();
                buf.extend_from_slice(first);
                buf.extend_from_slice(second);
                chunk.commit_all();
                buf
            }
            None => return Ok(String::new()),
        }
    };

    log::info!(
        "audio: captured {} samples ({:.1}s at 16kHz)",
        samples.len(),
        samples.len() as f64 / 16000.0
    );

    let samples = {
        let mut vad = crate::vad::VadFilter::new();
        let filtered = vad.filter_speech(&samples);
        if filtered.is_empty() {
            return Err("No speech detected in audio".to_string());
        }
        log::info!(
            "vad: {}/{} samples retained ({:.0}%)",
            filtered.len(),
            samples.len(),
            filtered.len() as f64 / samples.len() as f64 * 100.0
        );
        filtered
    };

    let _ = app.emit(
        "session:state",
        SessionInfo {
            id: String::new(),
            stage: SessionStage::Transcribing,
        },
    );

    // Pull current Whisper initial prompt from settings (vocabulary bias).
    // Failure to read is non-fatal: ASR works without a prompt.
    let whisper_prompt: String = {
        let guard = db_state.db.lock().await;
        guard
            .as_ref()
            .and_then(|db| db.get_app_settings().ok())
            .map(|s| s.whisper_initial_prompt)
            .or_else(|| {
                load_json_store(&app)
                    .ok()
                    .map(|store| store.settings.whisper_initial_prompt)
            })
            .unwrap_or_default()
    };

    // Transcribe (CPU-blocking, must use spawn_blocking)
    let whisper_arc: Arc<WhisperAsr> = {
        let guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
        match guard.as_ref() {
            Some(whisper) => Arc::clone(whisper),
            None => return Err("Whisper model not loaded".to_string()),
        }
    };
    let transcript = tokio::task::spawn_blocking(move || {
        let ctx = &whisper_arc.ctx;
        let mut params =
            whisper_rs::FullParams::new(whisper_rs::SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some("ja"));
        // Defence-in-depth: even if old DB rows contain '\0' (pre-sanitise),
        // never hand them to whisper_rs — it will panic. Empty string after
        // filtering is a no-op (skip set_initial_prompt entirely).
        let sanitised: String = whisper_prompt.chars().filter(|c| *c != '\0').collect();
        if !sanitised.is_empty() {
            params.set_initial_prompt(&sanitised);
        }
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);

        let mut state = ctx
            .create_state()
            .map_err(|e| format!("whisper state: {e}"))?;
        state
            .full(params, &samples)
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
pub async fn inject_text(app: AppHandle, text: String, mode: Option<String>) -> Result<(), String> {
    let inject_mode = match mode.as_deref() {
        Some("direct") => InjectMode::Direct,
        _ => InjectMode::Clipboard,
    };

    // enigo's macOS backend calls HIToolbox APIs (TSMGetInputSourceProperty,
    // islGetInputSourceListWithAdditions) which assert main-thread. Tauri
    // dispatches #[command] async fns onto tokio worker threads, so the
    // assertion trips and crashes the process. Hop the entire injection onto
    // the main NSRunLoop via Tauri's run_on_main_thread.
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let result = TextInjector::inject(&text, inject_mode);
        let _ = tx.send(result);
    })
    .map_err(|e| format!("run_on_main_thread dispatch: {e}"))?;

    rx.await
        .map_err(|e| format!("inject result channel closed: {e}"))?
        .map_err(|e| format!("{e:#}"))
}

/// Copy text to the system clipboard without synthesising any paste shortcut.
/// Used by the arm-and-paste flow: copy first, then wait for the user to
/// move focus, then synth Cmd+V.
#[tauri::command]
pub async fn set_clipboard_text(text: String) -> Result<(), String> {
    TextInjector::set_clipboard(&text).map_err(|e| format!("{e:#}"))
}

/// Synthesise the Cmd/Ctrl+V paste shortcut on whatever app currently has
/// focus. Hopped onto the main thread because enigo's macOS HIToolbox path
/// asserts main-thread.
#[tauri::command]
pub async fn synth_paste(app: AppHandle) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.run_on_main_thread(move || {
        let _ = tx.send(TextInjector::synth_paste());
    })
    .map_err(|e| format!("run_on_main_thread dispatch: {e}"))?;
    rx.await
        .map_err(|e| format!("synth_paste channel closed: {e}"))?
        .map_err(|e| format!("{e:#}"))
}

/// Whether the current frontmost app is something other than Dictation.
/// Cached read from the focus tracker; fresh within ~250ms.
#[tauri::command]
pub fn is_external_focused_now() -> bool {
    is_external_focused()
}

#[derive(Debug, Clone, Serialize)]
pub struct FocusedContext {
    pub text: String,
    pub truncated: bool,
}

impl From<FocusedFieldContext> for FocusedContext {
    fn from(c: FocusedFieldContext) -> Self {
        Self {
            text: c.text,
            truncated: c.truncated,
        }
    }
}

#[tauri::command]
pub async fn get_focused_context() -> Result<Option<FocusedContext>, String> {
    // AX calls are fast but synchronous; isolate from the runtime thread.
    tokio::task::spawn_blocking(|| get_focused_field_context().map(Into::into))
        .await
        .map_err(|e| format!("join: {e}"))
}

/// Build the rewrite prompt by filling `{input}`, `{context}`, and
/// `{dictionary}` placeholders. The dictionary slot is left empty in Phase A;
/// Phase B1 wires the SQLCipher-backed dictionary in.
#[tauri::command]
pub fn build_rewrite_prompt(
    template: String,
    input: String,
    context: Option<String>,
    dictionary: Option<String>,
) -> String {
    let context_block = context
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("\n参考(現在の入力欄の内容):\n{s}\n"))
        .unwrap_or_default();
    let dictionary_block = dictionary
        .filter(|s| !s.trim().is_empty())
        .map(|s| format!("\n辞書(以下の表記を尊重してください):\n{s}\n"))
        .unwrap_or_default();

    template
        .replace("{input}", &input)
        .replace("{context}", &context_block)
        .replace("{dictionary}", &dictionary_block)
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
        None => Ok(Vec::new()),
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
        None => Ok(Vec::new()),
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
    pub ax_trusted: bool,
    pub ready: bool,
}

#[tauri::command]
pub async fn check_setup(
    app: AppHandle,
    state: State<'_, LlmState>,
    db_state: State<'_, DbState>,
) -> Result<SetupStatus, String> {
    let mut status = SetupStatus {
        ollama_running: false,
        ollama_version: None,
        models_installed: vec![],
        models_missing: vec![],
        whisper_available: false,
        ax_trusted: is_ax_trusted(),
        ready: false,
    };

    // Try to read user setting up-front so we can short-circuit the LLM
    // checks if the user opted into Whisper-only mode. Failure to read is
    // non-fatal — fall through to the normal LLM-required gate.
    let bypass_llm: bool = {
        let guard = db_state.db.lock().await;
        match guard.as_ref() {
            Some(db) => db.get_app_settings().map(|s| s.bypass_llm).unwrap_or_else(|e| {
                log::warn!("check_setup: get_app_settings failed ({e:#}), defaulting to bypass_llm=true");
                true
            }),
            None => {
                load_json_store(&app)
                    .map(|store| store.settings.bypass_llm)
                    .unwrap_or_else(|e| {
                        log::warn!("check_setup: JSON settings unavailable ({e}), defaulting to bypass_llm=true");
                        true
                    })
            }
        }
    };

    // Check Ollama (still useful info even in bypass mode for the settings UI)
    match reqwest::get("http://127.0.0.1:11434/api/version").await {
        Ok(resp) => {
            status.ollama_running = true;
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                status.ollama_version = body["version"].as_str().map(|s| s.to_string());
            }
        }
        Err(_) => {
            // In LLM mode this is a hard stop; in bypass mode the user
            // explicitly doesn't need Ollama, so continue checking Whisper.
            if !bypass_llm {
                return Ok(status);
            }
        }
    }

    // Check models (only meaningful when Ollama is reachable)
    if status.ollama_running {
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
    } else {
        status.models_missing = REQUIRED_MODELS.iter().map(|s| s.to_string()).collect();
    }

    // Check whisper model
    status.whisper_available = resolve_whisper_model_path(&app, "small").is_some();

    // Setup is "ready" if AT LEAST ONE candidate is installed — the user
    // doesn't need every fallback. In Whisper-only (bypass_llm) mode the
    // LLM requirement is dropped entirely; only the whisper model matters.
    status.ready = if bypass_llm {
        status.whisper_available
    } else {
        status.ollama_running && !status.models_installed.is_empty() && status.whisper_available
    };

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
                let _ = app.emit(
                    "model:pull:progress",
                    serde_json::json!({
                        "model": &model,
                        "status": status,
                        "total": total,
                        "completed": completed,
                    }),
                );
            }
        }
    }

    let _ = app.emit("model:pull:done", &model);
    Ok(())
}

// ----- Dictionary commands -----

#[tauri::command]
pub async fn list_dictionary(
    app: AppHandle,
    state: State<'_, DbState>,
) -> Result<Vec<DictionaryEntry>, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.list_dictionary().map_err(|e| format!("{e:#}")),
        None => Ok(load_json_store(&app)?.dictionary),
    }
}

#[tauri::command]
pub async fn upsert_dictionary_entry(
    app: AppHandle,
    state: State<'_, DbState>,
    payload: DictionaryUpsert,
) -> Result<DictionaryEntry, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.upsert_dictionary(&payload).map_err(|e| format!("{e:#}")),
        None => {
            if payload.term.trim().is_empty() {
                return Err("term must not be empty".into());
            }
            let mut store = load_json_store(&app)?;
            let now = now_timestamp();
            let id = payload
                .id
                .clone()
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let existing_created_at = store
                .dictionary
                .iter()
                .find(|entry| entry.id == id)
                .map(|entry| entry.created_at.clone())
                .unwrap_or_else(|| now.clone());
            let entry = DictionaryEntry {
                id: id.clone(),
                term: payload.term,
                reading: payload.reading,
                aliases: payload.aliases,
                category: payload.category,
                notes: payload.notes,
                created_at: existing_created_at,
                updated_at: now,
            };
            if let Some(pos) = store
                .dictionary
                .iter()
                .position(|existing| existing.id == id)
            {
                store.dictionary[pos] = entry.clone();
            } else {
                store.dictionary.push(entry.clone());
            }
            store.dictionary.sort_by(|a, b| a.term.cmp(&b.term));
            save_json_store(&app, &store)?;
            Ok(entry)
        }
    }
}

#[tauri::command]
pub async fn delete_dictionary_entry(
    app: AppHandle,
    state: State<'_, DbState>,
    id: String,
) -> Result<(), String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.delete_dictionary(&id).map_err(|e| format!("{e:#}")),
        None => {
            let mut store = load_json_store(&app)?;
            store.dictionary.retain(|entry| entry.id != id);
            save_json_store(&app, &store)
        }
    }
}

// ----- Prompt-template commands -----

#[tauri::command]
pub async fn list_prompts(
    app: AppHandle,
    state: State<'_, DbState>,
) -> Result<Vec<PromptTemplate>, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.list_prompt_templates().map_err(|e| format!("{e:#}")),
        None => Ok(list_json_prompts(&load_json_store(&app)?)),
    }
}

#[tauri::command]
pub async fn upsert_prompt(
    app: AppHandle,
    state: State<'_, DbState>,
    payload: PromptTemplateUpsert,
) -> Result<PromptTemplate, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db
            .upsert_prompt_template(&payload)
            .map_err(|e| format!("{e:#}")),
        None => {
            if !payload.body.contains("{input}") {
                return Err("prompt template must contain {input} placeholder".into());
            }
            if payload.name.trim().is_empty() {
                return Err("name must not be empty".into());
            }
            let mut store = load_json_store(&app)?;
            let id = payload
                .id
                .clone()
                .unwrap_or_else(|| format!("user_{}", uuid::Uuid::new_v4()));
            let now = now_timestamp();
            let builtin = builtin_prompt_templates()
                .into_iter()
                .find(|prompt| prompt.id == id);
            let created_at = store
                .prompts
                .iter()
                .find(|prompt| prompt.id == id)
                .map(|prompt| prompt.created_at.clone())
                .or_else(|| builtin.as_ref().map(|prompt| prompt.created_at.clone()))
                .unwrap_or_else(|| now.clone());
            let prompt = PromptTemplate {
                id: id.clone(),
                name: payload.name,
                label: payload.label,
                body: payload.body,
                language: payload.language,
                is_builtin: builtin
                    .as_ref()
                    .map(|prompt| prompt.is_builtin)
                    .unwrap_or(false),
                order_idx: builtin
                    .as_ref()
                    .map(|prompt| prompt.order_idx)
                    .unwrap_or(99),
                created_at,
                updated_at: now,
            };
            if let Some(pos) = store.prompts.iter().position(|existing| existing.id == id) {
                store.prompts[pos] = prompt.clone();
            } else {
                store.prompts.push(prompt.clone());
            }
            save_json_store(&app, &store)?;
            Ok(prompt)
        }
    }
}

#[tauri::command]
pub async fn delete_prompt(
    app: AppHandle,
    state: State<'_, DbState>,
    id: String,
) -> Result<(), String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.delete_prompt_template(&id).map_err(|e| format!("{e:#}")),
        None => {
            if id.starts_with("builtin_") {
                return Err("cannot delete built-in template; use reset instead".into());
            }
            let mut store = load_json_store(&app)?;
            store.prompts.retain(|prompt| prompt.id != id);
            save_json_store(&app, &store)
        }
    }
}

#[tauri::command]
pub async fn reset_prompt(
    app: AppHandle,
    state: State<'_, DbState>,
    id: String,
) -> Result<PromptTemplate, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db
            .reset_prompt_template(&id, BUILTIN_PROMPTS)
            .map_err(|e| format!("{e:#}")),
        None => {
            let mut store = load_json_store(&app)?;
            store.prompts.retain(|prompt| prompt.id != id);
            save_json_store(&app, &store)?;
            builtin_prompt_templates()
                .into_iter()
                .find(|prompt| prompt.id == id)
                .ok_or_else(|| format!("no shipped default for builtin id '{id}'"))
        }
    }
}

#[tauri::command]
pub async fn get_autostart(app: AppHandle) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    app.autolaunch().is_enabled().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn set_autostart(app: AppHandle, enabled: bool) -> Result<bool, String> {
    use tauri_plugin_autostart::ManagerExt;
    let mgr = app.autolaunch();
    if enabled {
        mgr.enable().map_err(|e| format!("{e:#}"))?;
    } else {
        mgr.disable().map_err(|e| format!("{e:#}"))?;
    }
    mgr.is_enabled().map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn get_app_settings(
    app: AppHandle,
    state: State<'_, DbState>,
) -> Result<AppSettings, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.get_app_settings().map_err(|e| format!("{e:#}")),
        None => Ok(load_json_store(&app)?.settings),
    }
}

#[tauri::command]
pub async fn update_app_settings(
    app: AppHandle,
    state: State<'_, DbState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let normalized_hotkey =
        crate::hotkey::normalize_dictation_hotkey_id(&settings.dictation_hotkey);
    crate::hotkey::register_dictation_hotkey(&app, &normalized_hotkey)?;

    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => {
            let settings = AppSettings {
                dictation_hotkey: normalized_hotkey,
                ..settings
            };
            db.update_app_settings(&settings)
                .map_err(|e| format!("{e:#}"))?;
            db.get_app_settings().map_err(|e| format!("{e:#}"))
        }
        None => {
            let mut store = load_json_store(&app)?;
            let prompt: String = settings
                .whisper_initial_prompt
                .chars()
                .filter(|c| *c != '\0')
                .take(700)
                .collect();
            store.settings = AppSettings {
                whisper_initial_prompt: prompt,
                dictation_hotkey: normalized_hotkey,
                ..settings
            };
            save_json_store(&app, &store)?;
            Ok(store.settings)
        }
    }
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub struct DictionaryCandidate {
    pub term: String,
    pub reading: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Ask the configured LLM to extract dictionary candidates from a recent
/// ASR-vs-rewrite pair. The user can then check the ones they want and bulk
/// add them — much faster than hand-typing each entry.
#[tauri::command]
pub async fn generate_dictionary_candidates(
    state: State<'_, LlmState>,
    model: String,
    asr_text: String,
    rewritten_text: String,
) -> Result<Vec<DictionaryCandidate>, String> {
    let prompt = format!(
        "あなたは音声認識ユーザーの辞書登録を補助するアシスタントです。\n\
         以下の「ASR出力」と「修正後」から、辞書として登録する価値のある\n\
         候補を抽出してください。\n\
         \n\
         抽出ルール(以下のいずれかに該当するものを候補化):\n\
         1. 修正で表記が変わった固有名詞 → term=修正後、aliases=[ASR側の表記]\n\
         2. 修正で変わらなくても、ASRに出てくる人名・社名・地名・製品名・略語・\n\
            専門用語(将来 誤認識される可能性のある語)→ term=その表記、aliases=[]\n\
         3. 一般的な日本語(普通名詞、動詞、敬語整形、空白・句読点)は無視\n\
         \n\
         例:\n\
         - 入力「田中さんと打ち合わせ」→ 「田中」は普通名詞扱いで除外可。\n\
           ただし「ARI」「OpenAI」「Tauri」のような社名・固有名は必ず候補に。\n\
         - ASRで「元橋」、修正後で「本橋」→ {{\"term\":\"本橋\",\"aliases\":[\"元橋\"]}}\n\
         - ASRで「AR Advanced Technology」(社名)→ 修正不要でも候補に\n\
           {{\"term\":\"AR Advanced Technology\",\"aliases\":[]}}\n\
         \n\
         出力(JSON 配列のみ。コードブロック・説明文・前置き禁止):\n\
         [{{\"term\":\"...\",\"reading\":\"任意\",\"aliases\":[]}}]\n\
         該当なしなら []\n\
         \n\
         ASR出力:\n{}\n\n\
         修正後:\n{}\n\n\
         JSON:",
        asr_text, rewritten_text
    );

    let raw = state
        .runtime
        .rewrite(RewriteParams {
            model,
            prompt,
            max_new_tokens: 512,
            keep_alive: None,
        })
        .await
        .map_err(|e| format!("{e:#}"))?;

    let trimmed = raw.trim();
    let json_start = trimmed
        .find('[')
        .ok_or_else(|| format!("no JSON array in LLM output: {trimmed}"))?;
    let json_end = trimmed
        .rfind(']')
        .ok_or_else(|| format!("no JSON array end in LLM output: {trimmed}"))?
        + 1;
    let json_str = &trimmed[json_start..json_end];

    serde_json::from_str::<Vec<DictionaryCandidate>>(json_str)
        .map_err(|e| format!("parse dictionary candidates: {e} — body: {json_str}"))
}

/// Build a dictionary block by extracting only entries whose `term`,
/// `reading`, or `aliases` actually appear in the input/context. This avoids
/// shipping the entire dictionary into every prompt — long contexts hurt
/// small-LLM Japanese quality more than missing rare terms do.
#[tauri::command]
pub async fn extract_dictionary_block(
    app: AppHandle,
    state: State<'_, DbState>,
    input: String,
    context: Option<String>,
) -> Result<String, String> {
    let entries = {
        let guard = state.db.lock().await;
        match guard.as_ref() {
            Some(db) => db.list_dictionary().map_err(|e| format!("{e:#}"))?,
            None => load_json_store(&app)?.dictionary,
        }
    };
    Ok(format_relevant_dictionary(
        &entries,
        &input,
        context.as_deref(),
    ))
}

/// Pure helper, kept testable. Returns a multi-line bullet list or empty.
pub fn format_relevant_dictionary(
    entries: &[DictionaryEntry],
    input: &str,
    context: Option<&str>,
) -> String {
    let haystack: String = match context {
        Some(c) => format!("{input}\n{c}"),
        None => input.to_string(),
    };
    let mut lines: Vec<String> = Vec::new();
    for e in entries {
        let mut needles: Vec<&str> = Vec::with_capacity(2 + e.aliases.len());
        if !e.term.is_empty() {
            needles.push(&e.term);
        }
        if let Some(r) = e.reading.as_deref() {
            if !r.is_empty() {
                needles.push(r);
            }
        }
        for a in &e.aliases {
            if !a.is_empty() {
                needles.push(a);
            }
        }
        let hit = needles.iter().any(|n| haystack.contains(n));
        if !hit {
            continue;
        }
        let reading = e
            .reading
            .as_deref()
            .filter(|r| !r.is_empty())
            .map(|r| format!("({r})"))
            .unwrap_or_default();
        let category = e
            .category
            .as_deref()
            .filter(|c| !c.is_empty())
            .map(|c| format!(" [{c}]"))
            .unwrap_or_default();
        let notes = e
            .notes
            .as_deref()
            .filter(|n| !n.is_empty())
            .map(|n| format!(" — {n}"))
            .unwrap_or_default();
        lines.push(format!("- {}{reading}{category}{notes}", e.term));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_rewrite_prompt_substitutes_input() {
        let out =
            build_rewrite_prompt("INPUT={input}".to_string(), "hello".to_string(), None, None);
        assert_eq!(out, "INPUT=hello");
    }

    #[test]
    fn build_rewrite_prompt_inserts_context_block_when_present() {
        let out = build_rewrite_prompt(
            "{context}{input}".to_string(),
            "hi".to_string(),
            Some("こんにちは".into()),
            None,
        );
        assert!(out.contains("参考(現在の入力欄の内容):"));
        assert!(out.contains("こんにちは"));
        assert!(out.ends_with("hi"));
    }

    #[test]
    fn build_rewrite_prompt_skips_empty_context_and_dictionary() {
        let out = build_rewrite_prompt(
            "[c={context}][d={dictionary}][in={input}]".to_string(),
            "x".to_string(),
            Some("   ".into()),
            None,
        );
        assert_eq!(out, "[c=][d=][in=x]");
    }

    #[test]
    fn build_rewrite_prompt_inserts_dictionary_block() {
        let out = build_rewrite_prompt(
            "{dictionary}{input}".to_string(),
            "x".to_string(),
            None,
            Some("- 本橋(もとはし)".into()),
        );
        assert!(out.contains("辞書(以下の表記を尊重してください):"));
        assert!(out.contains("本橋(もとはし)"));
    }

    fn dict(term: &str, reading: Option<&str>, aliases: &[&str]) -> DictionaryEntry {
        DictionaryEntry {
            id: "x".into(),
            term: term.into(),
            reading: reading.map(String::from),
            aliases: aliases.iter().map(|s| s.to_string()).collect(),
            category: None,
            notes: None,
            created_at: "".into(),
            updated_at: "".into(),
        }
    }

    #[test]
    fn extract_dictionary_skips_unmatched_entries() {
        let entries = vec![
            dict("本橋", Some("もとはし"), &["元橋"]),
            dict("ARI", None, &[]),
        ];
        // Input mentions only one of them — the other should be skipped.
        let out = format_relevant_dictionary(&entries, "今日は元橋さんと話しました", None);
        assert!(out.contains("本橋"));
        assert!(!out.contains("ARI"));
    }

    #[test]
    fn extract_dictionary_uses_context_haystack() {
        let entries = vec![dict("Vite", None, &[])];
        let out =
            format_relevant_dictionary(&entries, "build setup", Some("we ship a Vite project"));
        assert!(out.contains("Vite"));
    }

    #[test]
    fn extract_dictionary_returns_empty_when_no_match() {
        let entries = vec![dict("本橋", Some("もとはし"), &["元橋"])];
        let out = format_relevant_dictionary(&entries, "hello world", None);
        assert!(out.is_empty());
    }

    #[test]
    fn extract_dictionary_includes_category_and_notes() {
        let mut e = dict("ARI", None, &["アリ"]);
        e.category = Some("company".into());
        e.notes = Some("親会社".into());
        let out = format_relevant_dictionary(&[e], "今日 ARI に行く", None);
        assert!(out.contains("ARI"));
        assert!(out.contains("[company]"));
        assert!(out.contains("親会社"));
    }

    #[test]
    fn extract_dictionary_skips_blank_aliases_and_reading() {
        // Defensive: blank strings shouldn't hit on every input via .contains("").
        let mut e = dict("Tauri", Some(""), &["", "tauri"]);
        e.category = Some("".into());
        let out = format_relevant_dictionary(&[e], "hello world", None);
        assert!(out.is_empty(), "blank alias must not match arbitrary text");
    }

    #[test]
    fn build_rewrite_prompt_template_without_placeholders_passes_through() {
        // Regression: a custom user template might omit {context}/{dictionary}.
        // Output should not crash; missing placeholders simply mean those
        // values are silently dropped.
        let out = build_rewrite_prompt(
            "no placeholders".to_string(),
            "x".to_string(),
            Some("ctx".into()),
            Some("dict".into()),
        );
        assert_eq!(out, "no placeholders");
    }

    #[test]
    fn build_rewrite_prompt_replaces_multiple_input_occurrences() {
        let out = build_rewrite_prompt(
            "echo {input} again {input}".to_string(),
            "hi".to_string(),
            None,
            None,
        );
        assert_eq!(out, "echo hi again hi");
    }

    #[test]
    fn json_prompts_include_builtins_when_store_is_empty() {
        let store = JsonFallbackStore::default();
        let prompts = list_json_prompts(&store);
        assert!(prompts.iter().any(|prompt| prompt.name == "ja_keigo"));
        assert!(prompts.iter().all(|prompt| prompt.is_builtin));
    }

    #[test]
    fn json_prompts_preserve_builtin_metadata_for_overrides() {
        let mut overridden = builtin_prompt_templates()
            .into_iter()
            .find(|prompt| prompt.name == "ja_keigo")
            .expect("ja_keigo builtin");
        overridden.label = "Edited".into();
        overridden.body = "Edited {input}".into();
        overridden.is_builtin = false;
        overridden.order_idx = 99;

        let store = JsonFallbackStore {
            prompts: vec![overridden],
            ..JsonFallbackStore::default()
        };
        let prompts = list_json_prompts(&store);
        let edited = prompts
            .iter()
            .find(|prompt| prompt.name == "ja_keigo")
            .expect("edited builtin");
        assert_eq!(edited.label, "Edited");
        assert_eq!(edited.body, "Edited {input}");
        assert!(edited.is_builtin);
        assert_eq!(edited.order_idx, 0);
    }
}
