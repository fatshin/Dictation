use crate::asr::AsrState;
use crate::db::{DbState, RewriteRecord};
use crate::inject::{InjectMode, TextInjector};
use crate::llm::{LlmState, ModelInfo, RewriteParams};
use crate::session::{DictationSession, SessionInfo, SessionStage, SessionState};
use tauri::{AppHandle, Emitter, State};
use tokio::sync::mpsc;

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
    _asr_state: State<'_, AsrState>,
) -> Result<String, String> {
    if !session_state
        .consent_given
        .load(std::sync::atomic::Ordering::Relaxed)
    {
        return Err("Recording consent not granted".to_string());
    }

    let mut guard = session_state.current.lock().await;
    if guard.is_some() {
        return Err("Session already active".to_string());
    }

    let session = DictationSession::new();
    let session_id = session.id.clone();
    *guard = Some(session);

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
) -> Result<(), String> {
    let mut guard = session_state.current.lock().await;
    if let Some(session) = guard.as_mut() {
        session.cancel.cancel();
        session.transition(SessionStage::Done);
        let _ = app.emit("session:state", session.info());
    }
    *guard = None;
    Ok(())
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
