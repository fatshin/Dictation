use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::keystore::SecretKey;

pub mod seed;
pub use seed::BUILTIN_PROMPTS;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryEntry {
    pub id: String,
    pub term: String,
    pub reading: Option<String>,
    /// JSON array of strings — common ASR misrecognitions to map to `term`.
    pub aliases: Vec<String>,
    /// Free-form, e.g. "person", "company", "tech", "phrase".
    pub category: Option<String>,
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DictionaryUpsert {
    /// None on create, Some on update.
    pub id: Option<String>,
    pub term: String,
    pub reading: Option<String>,
    pub aliases: Vec<String>,
    pub category: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplate {
    pub id: String,
    /// Stable, slug-style identifier (e.g. "ja_keigo"). Unique.
    pub name: String,
    /// Display label (UI-facing, can be edited).
    pub label: String,
    pub body: String,
    pub language: String,
    pub is_builtin: bool,
    pub order_idx: i32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptTemplateUpsert {
    pub id: Option<String>,
    pub name: String,
    pub label: String,
    pub body: String,
    pub language: String,
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
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY,
                applied_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS transcripts (
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

            CREATE TABLE IF NOT EXISTS dictionary (
                id TEXT PRIMARY KEY,
                term TEXT NOT NULL,
                reading TEXT,
                aliases TEXT NOT NULL DEFAULT '[]',
                category TEXT,
                notes TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS prompt_template (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL UNIQUE,
                label TEXT NOT NULL,
                body TEXT NOT NULL,
                language TEXT NOT NULL DEFAULT 'ja',
                is_builtin INTEGER NOT NULL DEFAULT 0,
                order_idx INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_transcripts_session ON transcripts(session_id);
            CREATE INDEX IF NOT EXISTS idx_rewrites_session ON rewrites(session_id);
            CREATE INDEX IF NOT EXISTS idx_transcripts_created ON transcripts(created_at);
            CREATE INDEX IF NOT EXISTS idx_rewrites_created ON rewrites(created_at);
            CREATE INDEX IF NOT EXISTS idx_dictionary_term ON dictionary(term);
            CREATE INDEX IF NOT EXISTS idx_dictionary_category ON dictionary(category);
            CREATE INDEX IF NOT EXISTS idx_prompt_template_lang ON prompt_template(language);
            INSERT OR IGNORE INTO schema_version (version) VALUES (1);
            INSERT OR IGNORE INTO schema_version (version) VALUES (2);",
        )?;
        Ok(())
    }

    /// Seed built-in prompt templates if missing. Idempotent: existing rows
    /// with the same `name` are left alone (so user edits are preserved).
    ///
    /// TODO(phase-b1.1): when we ship a new built-in body, INSERT OR IGNORE
    /// will skip the update on existing installs and users will be stuck on
    /// the old body until they hit "reset". Need a migration strategy: either
    /// store body_hash + force-overwrite when unedited, or bump
    /// schema_version with a one-shot REPLACE pass.
    pub fn seed_builtin_prompts(&self, defaults: &[(&str, &str, &str, &str)]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for (idx, (name, label, body, language)) in defaults.iter().enumerate() {
            let id = format!("builtin_{name}");
            tx.execute(
                "INSERT OR IGNORE INTO prompt_template
                 (id, name, label, body, language, is_builtin, order_idx)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6)",
                rusqlite::params![id, name, label, body, language, idx as i32],
            )?;
        }
        tx.commit()?;
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

    // ----- Dictionary CRUD -----

    pub fn list_dictionary(&self) -> Result<Vec<DictionaryEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, term, reading, aliases, category, notes, created_at, updated_at
             FROM dictionary ORDER BY term ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let aliases_json: String = row.get(3)?;
            Ok(DictionaryEntry {
                id: row.get(0)?,
                term: row.get(1)?,
                reading: row.get(2)?,
                aliases: serde_json::from_str(&aliases_json).unwrap_or_default(),
                category: row.get(4)?,
                notes: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn upsert_dictionary(&self, payload: &DictionaryUpsert) -> Result<DictionaryEntry> {
        if payload.term.trim().is_empty() {
            anyhow::bail!("term must not be empty");
        }
        let id = payload
            .id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let aliases_json = serde_json::to_string(&payload.aliases)?;
        let now = chrono_now();
        self.conn.execute(
            "INSERT INTO dictionary (id, term, reading, aliases, category, notes, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)
             ON CONFLICT(id) DO UPDATE SET
                 term=excluded.term,
                 reading=excluded.reading,
                 aliases=excluded.aliases,
                 category=excluded.category,
                 notes=excluded.notes,
                 updated_at=excluded.updated_at",
            rusqlite::params![
                id,
                payload.term,
                payload.reading,
                aliases_json,
                payload.category,
                payload.notes,
                now,
            ],
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT id, term, reading, aliases, category, notes, created_at, updated_at
             FROM dictionary WHERE id = ?1",
        )?;
        let row = stmt.query_row([&id], |row| {
            let aliases_json: String = row.get(3)?;
            Ok(DictionaryEntry {
                id: row.get(0)?,
                term: row.get(1)?,
                reading: row.get(2)?,
                aliases: serde_json::from_str(&aliases_json).unwrap_or_default(),
                category: row.get(4)?,
                notes: row.get(5)?,
                created_at: row.get(6)?,
                updated_at: row.get(7)?,
            })
        })?;
        Ok(row)
    }

    pub fn delete_dictionary(&self, id: &str) -> Result<()> {
        self.conn
            .execute("DELETE FROM dictionary WHERE id = ?1", [id])?;
        Ok(())
    }

    // ----- PromptTemplate CRUD -----

    pub fn list_prompt_templates(&self) -> Result<Vec<PromptTemplate>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, label, body, language, is_builtin, order_idx, created_at, updated_at
             FROM prompt_template ORDER BY order_idx ASC, name ASC",
        )?;
        let rows = stmt.query_map([], |row| {
            let is_builtin: i64 = row.get(5)?;
            Ok(PromptTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                label: row.get(2)?,
                body: row.get(3)?,
                language: row.get(4)?,
                is_builtin: is_builtin != 0,
                order_idx: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    pub fn upsert_prompt_template(
        &self,
        payload: &PromptTemplateUpsert,
    ) -> Result<PromptTemplate> {
        if !payload.body.contains("{input}") {
            anyhow::bail!("prompt template must contain {{input}} placeholder");
        }
        if payload.name.trim().is_empty() {
            anyhow::bail!("name must not be empty");
        }
        let id = payload
            .id
            .clone()
            .unwrap_or_else(|| format!("user_{}", uuid::Uuid::new_v4()));
        let now = chrono_now();
        self.conn.execute(
            "INSERT INTO prompt_template (id, name, label, body, language, is_builtin, order_idx, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 99, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 name=excluded.name,
                 label=excluded.label,
                 body=excluded.body,
                 language=excluded.language,
                 updated_at=excluded.updated_at",
            rusqlite::params![
                id,
                payload.name,
                payload.label,
                payload.body,
                payload.language,
                now,
            ],
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT id, name, label, body, language, is_builtin, order_idx, created_at, updated_at
             FROM prompt_template WHERE id = ?1",
        )?;
        let row = stmt.query_row([&id], |row| {
            let is_builtin: i64 = row.get(5)?;
            Ok(PromptTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                label: row.get(2)?,
                body: row.get(3)?,
                language: row.get(4)?,
                is_builtin: is_builtin != 0,
                order_idx: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        Ok(row)
    }

    pub fn delete_prompt_template(&self, id: &str) -> Result<()> {
        // Refuse to delete built-ins; reset_to_default replaces the body.
        let is_builtin: i64 = self
            .conn
            .query_row(
                "SELECT is_builtin FROM prompt_template WHERE id = ?1",
                [id],
                |r| r.get(0),
            )
            .unwrap_or(0);
        if is_builtin != 0 {
            anyhow::bail!("cannot delete built-in template; use reset instead");
        }
        self.conn
            .execute("DELETE FROM prompt_template WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Restore a built-in template's body to its shipped default (looked up by
    /// name in the provided `defaults` slice).
    pub fn reset_prompt_template(
        &self,
        id: &str,
        defaults: &[(&str, &str, &str, &str)],
    ) -> Result<PromptTemplate> {
        let name: String = self.conn.query_row(
            "SELECT name FROM prompt_template WHERE id = ?1 AND is_builtin = 1",
            [id],
            |r| r.get(0),
        )?;
        let (_, label, body, language) = defaults
            .iter()
            .find(|(n, _, _, _)| *n == name)
            .ok_or_else(|| anyhow::anyhow!("no shipped default for builtin '{name}'"))?;
        let now = chrono_now();
        self.conn.execute(
            "UPDATE prompt_template
             SET label = ?2, body = ?3, language = ?4, updated_at = ?5
             WHERE id = ?1",
            rusqlite::params![id, label, body, language, now],
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT id, name, label, body, language, is_builtin, order_idx, created_at, updated_at
             FROM prompt_template WHERE id = ?1",
        )?;
        let row = stmt.query_row([&id], |row| {
            let is_builtin: i64 = row.get(5)?;
            Ok(PromptTemplate {
                id: row.get(0)?,
                name: row.get(1)?,
                label: row.get(2)?,
                body: row.get(3)?,
                language: row.get(4)?,
                is_builtin: is_builtin != 0,
                order_idx: row.get(6)?,
                created_at: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        Ok(row)
    }
}

fn chrono_now() -> String {
    // UTC 'YYYY-MM-DD HH:MM:SS' to match SQLite's default `datetime('now')`,
    // which is also UTC. Aligns timestamps across schema-default and explicit
    // INSERT/UPDATE paths. Built manually to avoid pulling chrono.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Convert UTC seconds → broken-down date. Cheap, no DST.
    let secs_per_day = 86_400u64;
    let days = now / secs_per_day;
    let secs_in_day = now % secs_per_day;
    let h = secs_in_day / 3600;
    let m = (secs_in_day / 60) % 60;
    let s = secs_in_day % 60;
    let (y, mo, d) = days_to_ymd(days as i64);
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

fn days_to_ymd(days_since_epoch: i64) -> (i64, u32, u32) {
    // Civil-from-days, Howard Hinnant's algorithm.
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m as u32, d as u32)
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
