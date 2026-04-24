use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::Serialize;

use crate::keystore::SecretKey;

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptRecord {
    pub id: String,
    pub session_id: String,
    pub text: String,
    pub language: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RewriteRecord {
    pub id: String,
    pub session_id: String,
    pub input_text: String,
    pub output_text: String,
    pub model: String,
    pub template: String,
    pub created_at: String,
}

pub struct EncryptedDb {
    conn: Connection,
}

impl EncryptedDb {
    pub fn open(path: &std::path::Path, key: &SecretKey) -> Result<Self> {
        let conn = Connection::open(path)?;

        conn.pragma_update(None, "key", &format!("x'{}'", key.as_hex()))
            .context("PRAGMA key failed")?;
        conn.pragma_update(None, "cipher_page_size", 4096)
            .context("PRAGMA cipher_page_size failed")?;
        conn.pragma_update(None, "kdf_iter", 256000)
            .context("PRAGMA kdf_iter failed")?;
        conn.pragma_update(None, "cipher_memory_security", "ON")
            .context("PRAGMA cipher_memory_security failed")?;
        conn.pragma_update(None, "temp_store", "MEMORY")
            .context("PRAGMA temp_store failed")?;

        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS transcripts (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                text TEXT NOT NULL,
                language TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS rewrites (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                input_text TEXT NOT NULL,
                output_text TEXT NOT NULL,
                model TEXT NOT NULL,
                template TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_transcripts_session ON transcripts(session_id);
            CREATE INDEX IF NOT EXISTS idx_rewrites_session ON rewrites(session_id);
            CREATE INDEX IF NOT EXISTS idx_transcripts_created ON transcripts(created_at);
            CREATE INDEX IF NOT EXISTS idx_rewrites_created ON rewrites(created_at);",
        )?;
        Ok(())
    }

    pub fn insert_transcript(&self, record: &TranscriptRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO transcripts (id, session_id, text, language, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![
                record.id,
                record.session_id,
                record.text,
                record.language,
                record.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn insert_rewrite(&self, record: &RewriteRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO rewrites (id, session_id, input_text, output_text, model, template, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            rusqlite::params![
                record.id,
                record.session_id,
                record.input_text,
                record.output_text,
                record.model,
                record.template,
                record.created_at,
            ],
        )?;
        Ok(())
    }

    pub fn list_rewrites(&self, limit: usize) -> Result<Vec<RewriteRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, input_text, output_text, model, template, created_at
             FROM rewrites ORDER BY created_at DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(RewriteRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                input_text: row.get(2)?,
                output_text: row.get(3)?,
                model: row.get(4)?,
                template: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn search_rewrites(&self, query: &str, limit: usize) -> Result<Vec<RewriteRecord>> {
        let pattern = format!("%{query}%");
        let mut stmt = self.conn.prepare(
            "SELECT id, session_id, input_text, output_text, model, template, created_at
             FROM rewrites WHERE input_text LIKE ?1 OR output_text LIKE ?1
             ORDER BY created_at DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(rusqlite::params![pattern, limit], |row| {
            Ok(RewriteRecord {
                id: row.get(0)?,
                session_id: row.get(1)?,
                input_text: row.get(2)?,
                output_text: row.get(3)?,
                model: row.get(4)?,
                template: row.get(5)?,
                created_at: row.get(6)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

pub struct DbState {
    pub db: tokio::sync::Mutex<Option<EncryptedDb>>,
}

impl DbState {
    pub fn new() -> Self {
        Self {
            db: tokio::sync::Mutex::new(None),
        }
    }
}
