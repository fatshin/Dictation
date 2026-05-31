use serde::Serialize;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStage {
    Idle,
    ConsentPending,
    Recording,
    Transcribing,
    Rewriting,
    Injecting,
    Done,
    Error { stage: String, message: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub stage: SessionStage,
}

pub struct DictationSession {
    pub id: String,
    pub stage: SessionStage,
    pub cancel: CancellationToken,
    pub transcript: Option<String>,
    pub rewrite: Option<String>,
}

impl DictationSession {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            stage: SessionStage::Idle,
            cancel: CancellationToken::new(),
            transcript: None,
            rewrite: None,
        }
    }

    pub fn transition(&mut self, stage: SessionStage) {
        self.stage = stage;
    }

    pub fn info(&self) -> SessionInfo {
        SessionInfo {
            id: self.id.clone(),
            stage: self.stage.clone(),
        }
    }

    pub fn fail(&mut self, stage: &str, message: &str) {
        self.stage = SessionStage::Error {
            stage: stage.to_string(),
            message: message.to_string(),
        };
    }
}

impl Default for DictationSession {
    fn default() -> Self {
        Self::new()
    }
}

pub struct SessionState {
    pub current: tokio::sync::Mutex<Option<DictationSession>>,
    pub consent_given: std::sync::atomic::AtomicBool,
}

impl SessionState {
    pub fn new() -> Self {
        Self {
            current: tokio::sync::Mutex::new(None),
            consent_given: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

impl Default for SessionState {
    fn default() -> Self {
        Self::new()
    }
}
