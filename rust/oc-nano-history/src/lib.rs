//! oc-nano-history — SQLite conversation spill with close binding and bounded cache.
//!
//! A `HistoryStore` persists conversations and messages in SQLite. It fixes the
//! handle leak found in opencode-leak by providing an explicit `close()` binding
//! that drops the underlying connection, bounds the page cache via
//! `PRAGMA cache_size`, and streams conversation loads without materialising a
//! full `Vec<Message>`.

use std::path::PathBuf;
use std::sync::{Arc, AtomicBool, Mutex, Ordering};

use chrono::{TimeZone, Utc};
use rusqlite::Connection;
use serde_json::Value;

use oc_nano_core::{NanoError, NanoResult};

/// Embedded DDL for the `conversations` and `messages` tables plus indexes.
pub const SCHEMA_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS conversations (
    id TEXT PRIMARY KEY,
    title TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
    id TEXT PRIMARY KEY,
    conversation_id TEXT NOT NULL REFERENCES conversations(id),
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at);
"#;

/// Shared state behind an `Arc` so a `Handle` can be copied cheaply and
/// outlive any single borrow of the `HistoryStore`.
pub struct HistoryStoreInner {
    pub conn: Mutex<Connection>,
    pub db_path: PathBuf,
    pub cache_size_kb: i64,
    pub closed: AtomicBool,
}

/// Opaque, cloneable handle to a history store.
#[derive(Clone)]
pub struct Handle(pub Arc<HistoryStoreInner>);

/// SQLite-backed conversation store.
pub struct HistoryStore {
    pub inner: Arc<HistoryStoreInner>,
}

impl HistoryStore {
    /// Open (or create) the history database at `db_path`, creating parent
    /// directories as needed, then apply the embedded schema and a bounded page
    /// cache via `PRAGMA cache_size = -cache_size_kb` (negative = kibibytes).
    pub fn open(db_path: impl Into<PathBuf>, cache_size_kb: i64) -> NanoResult<HistoryStore> {
        let db_path = db_path.into();
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(map_err_to_rusqlite)?;
        }
        let conn = Connection::open(&db_path).map_err(map_err_to_rusqlite)?;
        conn.execute_batch(&format!("PRAGMA cache_size = {}", -cache_size_kb))
            .map_err(map_err_to_rusqlite)?;
        conn.execute_batch(SCHEMA_SQL).map_err(map_err_to_rusqlite)?;
        Ok(HistoryStore {
            inner: Arc::new(HistoryStoreInner {
                conn: Mutex::new(conn),
                db_path,
                cache_size_kb,
                closed: AtomicBool::new(false),
            }),
        })
    }

    /// Return a cloneable handle to this store.
    pub fn handle(&self) -> Handle {
        Handle(Arc::clone(&self.inner))
    }

    /// Close the handle: mark the store closed and drop the underlying
    /// connection. All subsequent operations on any handle to this store error.
    pub fn close(handle: &Handle) -> NanoResult<()> {
        handle.0.closed.store(true, Ordering::SeqCst);
        // Drop the real connection by swapping in a short-lived in-memory one.
        let mut lock = handle.0.conn.lock().unwrap();
        *lock = Connection::open_in_memory().map_err(map_err_to_rusqlite)?;
        Ok(())
    }

    /// Store a single JSON message by parsing it and inserting a row.
    pub fn store_message(&self, handle: &Handle, message_json: String) -> NanoResult<()> {
        if handle.0.closed.load(Ordering::SeqCst) {
            return Err(NanoError::Closed);
        }
        let value: Value = serde_json::from_str(&message_json).map_err(map_err_to_rusqlite)?;
        let id: &str = value.get("id").and_then(|v| v.as_str()).unwrap_or("");
        let conversation_id: &str = value
            .get("conversation_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let role: &str = value.get("role").and_then(|v| v.as_str()).unwrap_or("user");
        let content: &str = value.get("content").and_then(|v| v.as_str()).unwrap_or("");
        let created_at: &str = value
            .get("created_at")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let lock = self.inner.conn.lock().unwrap();
        lock.execute(
            "INSERT OR IGNORE INTO conversations (id, created_at, updated_at) VALUES (?1, ?2, ?2)",
            &[conversation_id, created_at],
        )
        .map_err(map_err_to_rusqlite)?;
        lock.execute(
            "INSERT OR REPLACE INTO messages (id, conversation_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
            &[id, conversation_id, role, content, created_at],
        )
        .map_err(map_err_to_rusqlite)?;
        Ok(())
    }

    /// Stream a conversation's messages directly into a JSON string without
    /// materialising a `Vec<Message>`.
    pub fn load_conversation_json(
        &self,
        handle: &Handle,
        conversation_id: String,
    ) -> NanoResult<String> {
        if handle.0.closed.load(Ordering::SeqCst) {
            return Err(NanoError::Closed);
        }
        let lock = self.inner.conn.lock().unwrap();
        let mut stmt = lock
            .prepare(
                "SELECT id, role, content, created_at FROM messages WHERE conversation_id = ?1 ORDER BY created_at ASC",
            )
            .map_err(map_err_to_rusqlite)?;
        let rows = stmt
            .query_map([&conversation_id], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .map_err(map_err_to_rusqlite)?;
        let mut out = String::from("[");
        let mut first = true;
        for row in rows {
            let (id, role, content, created_at) = row.map_err(map_err_to_rusqlite)?;
            if !first {
                out.push(',');
            }
            first = false;
            out.push_str("{\"id\":");
            out.push_str(&serde_json::to_string(&id).map_err(map_err_to_rusqlite)?);
            out.push_str(",\"role\":");
            out.push_str(&serde_json::to_string(&role).map_err(map_err_to_rusqlite)?);
            out.push_str(",\"content\":");
            out.push_str(&serde_json::to_string(&content).map_err(map_err_to_rusqlite)?);
            out.push_str(",\"created_at\":");
            out.push_str(&serde_json::to_string(&created_at).map_err(map_err_to_rusqlite)?);
            out.push('}');
        }
        out.push(']');
        Ok(out)
    }

    /// Transactional batch insert of a conversation's messages.
    pub fn spill_conversation(
        &self,
        handle: &Handle,
        conversation_id: String,
        messages_json: String,
    ) -> NanoResult<usize> {
        if handle.0.closed.load(Ordering::SeqCst) {
            return Err(NanoError::Closed);
        }
        let messages: Vec<Value> = serde_json::from_str(&messages_json).map_err(map_err_to_rusqlite)?;
        let now = Utc::now().to_rfc3339();
        let lock = self.inner.conn.lock().unwrap();
        let tx = lock.transaction().map_err(map_err_to_rusqlite)?;
        tx.execute(
            "INSERT OR IGNORE INTO conversations (id, created_at, updated_at) VALUES (?1, ?2, ?2)",
            &[conversation_id.as_str(), now.as_str()],
        )
        .map_err(map_err_to_rusqlite)?;
        let mut count: usize = 0;
        for msg in &messages {
            let id: &str = msg.get("id").and_then(|v| v.as_str()).unwrap_or("");
            let role: &str = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let content: &str = msg.get("content").and_then(|v| v.as_str()).unwrap_or("");
            let created_at: &str = msg
                .get("created_at")
                .and_then(|v| v.as_str())
                .unwrap_or(now.as_str());
            tx.execute(
                "INSERT OR REPLACE INTO messages (id, conversation_id, role, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                &[id, conversation_id.as_str(), role, content, created_at],
            )
            .map_err(map_err_to_rusqlite)?;
            count += 1;
        }
        tx.commit().map_err(map_err_to_rusqlite)?;
        Ok(count)
    }

    /// Delete conversations (and their messages) updated before `cutoff`.
    /// No `VACUUM` is run here.
    pub fn prune_before(&self, handle: &Handle, cutoff: String) -> NanoResult<usize> {
        if handle.0.closed.load(Ordering::SeqCst) {
            return Err(NanoError::Closed);
        }
        let lock = self.inner.conn.lock().unwrap();
        let tx = lock.transaction().map_err(map_err_to_rusqlite)?;
        let deleted_messages = tx
            .execute(
                "DELETE FROM messages WHERE conversation_id IN (SELECT id FROM conversations WHERE updated_at < ?1)",
                &[cutoff.as_str()],
            )
            .map_err(map_err_to_rusqlite)?;
        let deleted_conversations = tx
            .execute("DELETE FROM conversations WHERE updated_at < ?1", &[cutoff.as_str()])
            .map_err(map_err_to_rusqlite)?;
        tx.commit().map_err(map_err_to_rusqlite)?;
        Ok(deleted_messages + deleted_conversations)
    }

    /// Delete conversations older than `ttl_seconds` from `Utc::now()`.
    /// Runs `VACUUM` (only here, not on every `prune_before`).
    pub fn prune_old(&self, handle: &Handle, ttl_seconds: i64) -> NanoResult<usize> {
        if handle.0.closed.load(Ordering::SeqCst) {
            return Err(NanoError::Closed);
        }
        let cutoff = (Utc::now() - chrono::Duration::seconds(ttl_seconds)).to_rfc3339();
        let count = self.prune_before(handle, cutoff.clone())?;
        {
            let lock = self.inner.conn.lock().unwrap();
            lock.execute_batch("VACUUM").map_err(map_err_to_rusqlite)?;
        }
        Ok(count)
    }
}

/// Convert any error into a `NanoError::Other` for `rusqlite`-style conversion.
fn map_err_to_rusqlite<E: std::error::Error>(e: E) -> NanoError {
    NanoError::Other(format!("{e}"))
}
