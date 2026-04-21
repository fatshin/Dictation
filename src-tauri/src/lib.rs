//! Dictation backend library.
//!
//! Phase-1a scaffold per ADR-001 (whisper-rs ASR — pending) + ADR-002
//! (Ollama LLM — wired here). Modules are stub-first: each module ships a
//! trait and an in-memory or HTTP-backed impl so the frontend can be wired
//! and tested before all sidecar / DB code lands. Real implementations
//! arrive in subsequent slices, gated by the Phase-0 acceptance criteria
//! from `docs/PHASE0_POC.md` and the offline-egress security model in
//! `docs/SECURITY.md`.

pub mod commands;
pub mod llm;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(commands::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_models,
            commands::rewrite_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
