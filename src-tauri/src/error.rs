use serde::Serialize;

#[derive(Debug, thiserror::Error, Serialize, Clone)]
pub enum DictationError {
    #[error("LLM: {0}")]
    Llm(String),
    #[error("ASR: {0}")]
    Asr(String),
    #[error("Audio: {0}")]
    Audio(String),
    #[error("Injection: {0}")]
    Injection(String),
    #[error("Database: {0}")]
    Database(String),
    #[error("Keystore: {0}")]
    Keystore(String),
    #[error("Permission: {0}")]
    Permission(String),
    #[error("Session: {0}")]
    Session(String),
}