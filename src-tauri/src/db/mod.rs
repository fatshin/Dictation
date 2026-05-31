#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
#[cfg(target_os = "macos")]
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[cfg(target_os = "macos")]
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

/// Single-row global app settings. Persisted in `app_settings` table with
/// `id = 1` invariant. Add new fields here + bump migration block.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    /// Skip the LLM rewrite pipeline entirely. When true, the raw Whisper
    /// transcript is fed straight to the paste/inject path.
    pub bypass_llm: bool,
    /// Initial prompt passed to whisper_rs::FullParams::set_initial_prompt.
    /// Used for vocabulary biasing (proper nouns, technical terms, style).
    /// Whisper truncates to ~224 tokens internally, so keep this short.
    pub whisper_initial_prompt: String,
    /// Selected input device name. None = use system default.
    pub input_device: Option<String>,
    /// Whisper model ID: "small", "medium", "large-v3-turbo".
    pub whisper_model: String,
    /// Global shortcut preset used to toggle dictation recording.
    #[serde(default = "default_dictation_hotkey")]
    pub dictation_hotkey: String,
}

pub fn default_dictation_hotkey() -> String {
    crate::hotkey::DEFAULT_DICTATION_HOTKEY_ID.to_string()
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            bypass_llm: true,
            whisper_initial_prompt: String::new(),
            input_device: None,
            whisper_model: "small".to_string(),
            dictation_hotkey: default_dictation_hotkey(),
        }
    }
}

#[cfg(target_os = "macos")]
pub struct EncryptedDb {
    conn: Connection,
}

#[cfg(not(target_os = "macos"))]
pub struct EncryptedDb;

#[cfg(target_os = "macos")]
impl EncryptedDb {
    pub fn open(path: &std::path::Path, key: &SecretKey) -> Result<Self> {
        let conn = Connection::open(path)?;

        conn.pragma_update(None, "key", format!("x'{}'", key.as_hex()))
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
                body_hash TEXT,
                created_at TEXT NOT NULL DEFAULT (datetime('now')),
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE TABLE IF NOT EXISTS app_settings (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                bypass_llm INTEGER NOT NULL DEFAULT 0,
                whisper_initial_prompt TEXT NOT NULL DEFAULT '',
                input_device TEXT DEFAULT NULL,
                whisper_model TEXT NOT NULL DEFAULT 'small',
                dictation_hotkey TEXT NOT NULL DEFAULT 'primary_d',
                updated_at TEXT NOT NULL DEFAULT (datetime('now'))
            );

            CREATE INDEX IF NOT EXISTS idx_transcripts_session ON transcripts(session_id);
            CREATE INDEX IF NOT EXISTS idx_rewrites_session ON rewrites(session_id);
            CREATE INDEX IF NOT EXISTS idx_transcripts_created ON transcripts(created_at);
            CREATE INDEX IF NOT EXISTS idx_rewrites_created ON rewrites(created_at);
            CREATE INDEX IF NOT EXISTS idx_dictionary_term ON dictionary(term);
            CREATE INDEX IF NOT EXISTS idx_dictionary_category ON dictionary(category);
            CREATE INDEX IF NOT EXISTS idx_prompt_template_lang ON prompt_template(language);
            INSERT OR IGNORE INTO app_settings (id) VALUES (1);
            INSERT OR IGNORE INTO schema_version (version) VALUES (1);
            INSERT OR IGNORE INTO schema_version (version) VALUES (2);
            INSERT OR IGNORE INTO schema_version (version) VALUES (3);",
        )?;

        // v4: add input_device column for existing databases
        let has_input_device: bool = self
            .conn
            .prepare("PRAGMA table_info(app_settings)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|col| col == "input_device");
        if !has_input_device {
            self.conn.execute_batch(
                "ALTER TABLE app_settings ADD COLUMN input_device TEXT DEFAULT NULL;
                 INSERT OR IGNORE INTO schema_version (version) VALUES (4);",
            )?;
        }

        let has_whisper_model: bool = self
            .conn
            .prepare("PRAGMA table_info(app_settings)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|col| col == "whisper_model");
        if !has_whisper_model {
            self.conn.execute_batch(
                "ALTER TABLE app_settings ADD COLUMN whisper_model TEXT NOT NULL DEFAULT 'small';
                 INSERT OR IGNORE INTO schema_version (version) VALUES (5);",
            )?;
        }

        let has_dictation_hotkey: bool = self
            .conn
            .prepare("PRAGMA table_info(app_settings)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .any(|col| col == "dictation_hotkey");
        if !has_dictation_hotkey {
            self.conn.execute_batch(
                "ALTER TABLE app_settings ADD COLUMN dictation_hotkey TEXT NOT NULL DEFAULT 'primary_d';
                 INSERT OR IGNORE INTO schema_version (version) VALUES (6);",
            )?;
        }

        Ok(())
    }

    /// Seed built-in prompt templates. On first install, inserts all
    /// defaults. On subsequent launches, updates body+label only if the
    /// user has NOT manually edited the template (detected via body_hash).
    ///
    /// The `body_hash` column stores the SHA-256 of the *shipped* body at
    /// the time it was last seeded. When the user edits the body through
    /// the UI, `upsert_prompt_template` clears `body_hash` to NULL,
    /// signalling "user-modified — do not overwrite on next seed".
    pub fn seed_builtin_prompts(&self, defaults: &[(&str, &str, &str, &str)]) -> Result<()> {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        fn quick_hash(s: &str) -> String {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            format!("{:016x}", h.finish())
        }

        let tx = self.conn.unchecked_transaction()?;
        for (idx, (name, label, body, language)) in defaults.iter().enumerate() {
            let id = format!("builtin_{name}");
            let hash = quick_hash(body);
            // INSERT if new. If already exists AND body_hash matches the
            // previously-seeded hash (= user hasn't edited), update the
            // body+label to the new shipped version.
            // body_hash IS NOT NULL means the user hasn't edited the
            // body via the UI (upsert_prompt_template clears it to NULL
            // on user edits). Safe to overwrite with the new shipped body.
            tx.execute(
                "INSERT INTO prompt_template
                 (id, name, label, body, language, is_builtin, order_idx, body_hash)
                 VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7)
                 ON CONFLICT(id) DO UPDATE SET
                     label = excluded.label,
                     body = excluded.body,
                     language = excluded.language,
                     order_idx = excluded.order_idx,
                     body_hash = excluded.body_hash,
                     updated_at = datetime('now')
                 WHERE prompt_template.body_hash IS NOT NULL",
                rusqlite::params![id, name, label, body, language, idx as i32, hash],
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

    pub fn upsert_prompt_template(&self, payload: &PromptTemplateUpsert) -> Result<PromptTemplate> {
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
        // Clear body_hash on user edits so seed_builtin_prompts won't
        // overwrite the user's customisation on next launch.
        self.conn.execute(
            "INSERT INTO prompt_template (id, name, label, body, language, is_builtin, order_idx, body_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 0, 99, NULL, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 name=excluded.name,
                 label=excluded.label,
                 body=excluded.body,
                 language=excluded.language,
                 body_hash=NULL,
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
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut h = DefaultHasher::new();
        body.hash(&mut h);
        let hash = format!("{:016x}", h.finish());
        let now = chrono_now();
        self.conn.execute(
            "UPDATE prompt_template
             SET label = ?2, body = ?3, language = ?4, body_hash = ?5, updated_at = ?6
             WHERE id = ?1",
            rusqlite::params![id, label, body, language, hash, now],
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

    // ----- App settings (single-row) -----

    pub fn get_app_settings(&self) -> Result<AppSettings> {
        let row = self.conn.query_row(
            "SELECT bypass_llm, whisper_initial_prompt, input_device, whisper_model, dictation_hotkey
             FROM app_settings WHERE id = 1",
            [],
            |r| {
                let bypass: i64 = r.get(0)?;
                let prompt: String = r.get(1)?;
                let input_device: Option<String> = r.get(2)?;
                let whisper_model: String = r.get(3)?;
                let dictation_hotkey: String = r.get(4)?;
                Ok((bypass != 0, prompt, input_device, whisper_model, dictation_hotkey))
            },
        );
        match row {
            Ok((
                bypass_llm,
                whisper_initial_prompt,
                input_device,
                whisper_model,
                dictation_hotkey,
            )) => Ok(AppSettings {
                bypass_llm,
                whisper_initial_prompt,
                input_device,
                whisper_model,
                dictation_hotkey: crate::hotkey::normalize_dictation_hotkey_id(&dictation_hotkey),
            }),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(AppSettings::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn update_app_settings(&self, s: &AppSettings) -> Result<()> {
        let now = chrono_now();
        let prompt: String = s
            .whisper_initial_prompt
            .chars()
            .filter(|c| *c != '\0')
            .take(700)
            .collect();
        self.conn.execute(
            "INSERT INTO app_settings (id, bypass_llm, whisper_initial_prompt, input_device, whisper_model, dictation_hotkey, updated_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                bypass_llm = excluded.bypass_llm,
                whisper_initial_prompt = excluded.whisper_initial_prompt,
                input_device = excluded.input_device,
                whisper_model = excluded.whisper_model,
                dictation_hotkey = excluded.dictation_hotkey,
                updated_at = excluded.updated_at",
            rusqlite::params![
                s.bypass_llm as i64,
                prompt,
                s.input_device,
                s.whisper_model,
                crate::hotkey::normalize_dictation_hotkey_id(&s.dictation_hotkey),
                now
            ],
        )?;
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
impl EncryptedDb {
    pub fn get_app_settings(&self) -> Result<AppSettings> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn update_app_settings(&self, _s: &AppSettings) -> Result<()> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn list_dictionary(&self) -> Result<Vec<DictionaryEntry>> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn upsert_dictionary(&self, _payload: &DictionaryUpsert) -> Result<DictionaryEntry> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn delete_dictionary(&self, _id: &str) -> Result<()> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn list_prompt_templates(&self) -> Result<Vec<PromptTemplate>> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn upsert_prompt_template(
        &self,
        _payload: &PromptTemplateUpsert,
    ) -> Result<PromptTemplate> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn delete_prompt_template(&self, _id: &str) -> Result<()> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn reset_prompt_template(
        &self,
        _id: &str,
        _defaults: &[(&str, &str, &str, &str)],
    ) -> Result<PromptTemplate> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn list_rewrites(&self, _limit: usize) -> Result<Vec<RewriteRecord>> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }

    pub fn search_rewrites(&self, _query: &str, _limit: usize) -> Result<Vec<RewriteRecord>> {
        anyhow::bail!("SQLCipher DB unavailable on this platform")
    }
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
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

impl Default for DbState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{default_dictation_hotkey, AppSettings};

    #[test]
    fn app_settings_default_uses_default_dictation_hotkey() {
        let settings = AppSettings::default();
        assert_eq!(settings.dictation_hotkey, default_dictation_hotkey());
    }

    #[test]
    fn app_settings_deserializes_missing_dictation_hotkey() {
        let raw = r#"{
            "bypass_llm": true,
            "whisper_initial_prompt": "",
            "input_device": null,
            "whisper_model": "small"
        }"#;
        let settings: AppSettings = serde_json::from_str(raw).expect("legacy settings json");
        assert_eq!(settings.dictation_hotkey, default_dictation_hotkey());
    }
}
