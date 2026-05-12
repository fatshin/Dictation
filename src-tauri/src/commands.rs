use crate::asr::{resolve_whisper_model_path, AsrState, WhisperAsr};
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
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
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
        let model_path = resolve_whisper_model_path(&app)
            .ok_or_else(|| "Whisper model not found. Set DICTATION_WHISPER_MODEL or install to ~/Library/Application Support/Dictation/models/ggml-small.bin".to_string())?;
        let path_str = model_path.to_string_lossy().to_string();
        let asr = tokio::task::spawn_blocking(move || WhisperAsr::new(&path_str))
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
    db_state: State<'_, DbState>,
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
                let chunk = consumer
                    .read_chunk(available)
                    .map_err(|e| format!("read_chunk: {e:?}"))?;
                let (first, second) = chunk.as_slices();
                buf.extend_from_slice(first);
                buf.extend_from_slice(second);
                chunk.commit_all();
                buf
            }
            None => return Err("No audio consumer available".to_string()),
        }
    };

    log::info!(
        "audio: captured {} samples ({:.1}s at 16kHz)",
        samples.len(),
        samples.len() as f64 / 16000.0
    );

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
        guard
            .as_ref()
            .and_then(|db| db.get_app_settings().ok())
            .map(|s| s.bypass_llm)
            .unwrap_or(true)
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
    status.whisper_available = resolve_whisper_model_path(&app).is_some();

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
pub async fn list_dictionary(state: State<'_, DbState>) -> Result<Vec<DictionaryEntry>, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.list_dictionary().map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

#[tauri::command]
pub async fn upsert_dictionary_entry(
    state: State<'_, DbState>,
    payload: DictionaryUpsert,
) -> Result<DictionaryEntry, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.upsert_dictionary(&payload).map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

#[tauri::command]
pub async fn delete_dictionary_entry(state: State<'_, DbState>, id: String) -> Result<(), String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.delete_dictionary(&id).map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

// ----- Prompt-template commands -----

#[tauri::command]
pub async fn list_prompts(state: State<'_, DbState>) -> Result<Vec<PromptTemplate>, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.list_prompt_templates().map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

#[tauri::command]
pub async fn upsert_prompt(
    state: State<'_, DbState>,
    payload: PromptTemplateUpsert,
) -> Result<PromptTemplate, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db
            .upsert_prompt_template(&payload)
            .map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

#[tauri::command]
pub async fn delete_prompt(state: State<'_, DbState>, id: String) -> Result<(), String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.delete_prompt_template(&id).map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

#[tauri::command]
pub async fn reset_prompt(state: State<'_, DbState>, id: String) -> Result<PromptTemplate, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db
            .reset_prompt_template(&id, BUILTIN_PROMPTS)
            .map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

#[tauri::command]
pub async fn get_app_settings(state: State<'_, DbState>) -> Result<AppSettings, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db.get_app_settings().map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
}

#[tauri::command]
pub async fn update_app_settings(
    state: State<'_, DbState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => {
            db.update_app_settings(&settings)
                .map_err(|e| format!("{e:#}"))?;
            db.get_app_settings().map_err(|e| format!("{e:#}"))
        }
        None => Err("DB not initialized".into()),
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
    state: State<'_, DbState>,
    input: String,
    context: Option<String>,
) -> Result<String, String> {
    let entries = {
        let guard = state.db.lock().await;
        match guard.as_ref() {
            Some(db) => db.list_dictionary().map_err(|e| format!("{e:#}"))?,
            None => return Ok(String::new()),
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
}
