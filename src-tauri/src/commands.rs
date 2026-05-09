use crate::asr::{resolve_whisper_model_path, AsrState, WhisperAsr};
use crate::audio::AudioConfig;
use crate::db::{
    DbState, DictionaryEntry, DictionaryUpsert, PromptTemplate, PromptTemplateUpsert,
    RewriteRecord, BUILTIN_PROMPTS,
};
use crate::inject::{
    get_focused_field_context, is_ax_trusted, FocusedFieldContext, InjectMode, TextInjector,
};
use crate::llm::{LlmState, ModelInfo, RewriteParams};
use crate::session::{DictationSession, SessionInfo, SessionStage, SessionState};
use serde::Serialize;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

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
    let whisper_arc: Arc<WhisperAsr> = {
        let guard = asr_state.whisper.lock().map_err(|e| format!("{e}"))?;
        match guard.as_ref() {
            Some(whisper) => Arc::clone(whisper),
            None => return Err("Whisper model not loaded".to_string()),
        }
    };
    let transcript = tokio::task::spawn_blocking(move || {
        let ctx = &whisper_arc.ctx;
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
        Some("direct") => InjectMode::Direct,
        _ => InjectMode::Clipboard,
    };
    TextInjector::inject(&text, inject_mode).map_err(|e| format!("{e:#}"))
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
    status.whisper_available = resolve_whisper_model_path(&app).is_some();

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
pub async fn delete_dictionary_entry(
    state: State<'_, DbState>,
    id: String,
) -> Result<(), String> {
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
pub async fn reset_prompt(
    state: State<'_, DbState>,
    id: String,
) -> Result<PromptTemplate, String> {
    let guard = state.db.lock().await;
    match guard.as_ref() {
        Some(db) => db
            .reset_prompt_template(&id, BUILTIN_PROMPTS)
            .map_err(|e| format!("{e:#}")),
        None => Err("DB not initialized".into()),
    }
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
    Ok(format_relevant_dictionary(&entries, &input, context.as_deref()))
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
        let out = build_rewrite_prompt(
            "INPUT={input}".to_string(),
            "hello".to_string(),
            None,
            None,
        );
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
}
