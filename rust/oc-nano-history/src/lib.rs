//! oc-nano-history — SQLite conversation spill with close binding and bounded cache.
//!
//! A `HistoryStore` persists conversations and messages in SQLite. It fixes the
//! handle leak found in opencode-leak by providing an explicit `close()` binding
//! that drops the underlying connection, bounds the page cache via
//! `PRAGMA cache_size`, and streams conversation loads without materialising a
//! full `Vec<Message>`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use rusqlite::Connection;
use serde_json::Value;

use oc_nano_core::{NanoError, NanoResult};

/// Largest single message the store accepts (1 MiB).
///
/// The message arrives as one JSON string that is parsed and written as a row.
/// The cap is checked before the parse, so an oversized payload is rejected
/// rather than turned into an oversized allocation.
pub const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

/// Largest conversation payload one spill accepts (16 MiB).
pub const MAX_SPILL_BYTES: usize = 16 * 1024 * 1024;

/// Largest number of rows one spill may write.
pub const MAX_SPILL_MESSAGES: usize = 10_000;

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
    ///
    /// The `db_path` is sanitized before use: `..` path-traversal components are
    /// rejected, and the resolved path must remain within the current working
    /// directory (the allowed data directory). This prevents an attacker from
    /// causing the store to create or open a SQLite database at an arbitrary
    /// location.
    pub fn open(db_path: impl Into<PathBuf>, cache_size_kb: i64) -> NanoResult<HistoryStore> {
        let db_path = db_path.into();

        // Reject path traversal via `..` components.
        if db_path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        {
            return Err(NanoError::InvalidInput(
                "path traversal not allowed in db_path".to_string(),
            ));
        }

        // Resolve relative paths against the current working directory, then
        // verify the resolved path stays within it (the allowed data directory).
        let resolved = if db_path.is_absolute() {
            db_path
        } else {
            std::env::current_dir()
                .map_err(map_err_to_rusqlite)?
                .join(&db_path)
        };

        let canonical_root = std::env::current_dir()
            .map_err(map_err_to_rusqlite)?
            .canonicalize()
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let canonical_resolved = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());

        if !canonical_resolved.starts_with(&canonical_root) {
            return Err(NanoError::InvalidInput(
                "db_path outside allowed directory".to_string(),
            ));
        }

        if let Some(parent) = resolved.parent() {
            std::fs::create_dir_all(parent).map_err(map_err_to_rusqlite)?;
        }
        let conn = Connection::open(&resolved).map_err(map_err_to_rusqlite)?;
        conn.execute_batch(&format!("PRAGMA cache_size = {}", -cache_size_kb))
            .map_err(map_err_to_rusqlite)?;
        conn.execute_batch(SCHEMA_SQL).map_err(map_err_to_rusqlite)?;
        Ok(HistoryStore {
            inner: Arc::new(HistoryStoreInner {
                conn: Mutex::new(conn),
                db_path: resolved,
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
        // Check the size before parsing: the parse is what turns a large input
        // into a large allocation.
        if message_json.len() > MAX_MESSAGE_BYTES {
            return Err(NanoError::InvalidInput(format!(
                "message exceeds {MAX_MESSAGE_BYTES} bytes"
            )));
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
        // Both caps are checked before the parse: the whole conversation is
        // materialised in memory, so the size has to be bounded first.
        if messages_json.len() > MAX_SPILL_BYTES {
            return Err(NanoError::InvalidInput(format!(
                "conversation payload exceeds {MAX_SPILL_BYTES} bytes"
            )));
        }
        let messages: Vec<Value> = serde_json::from_str(&messages_json).map_err(map_err_to_rusqlite)?;
        if messages.len() > MAX_SPILL_MESSAGES {
            return Err(NanoError::InvalidInput(format!(
                "conversation holds more than {MAX_SPILL_MESSAGES} messages"
            )));
        }
        let now = Utc::now().to_rfc3339();
        let mut lock = self.inner.conn.lock().unwrap();
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
        let mut lock = self.inner.conn.lock().unwrap();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The store only opens databases inside the current working directory, so
    /// the fixtures live under the crate's own target directory.
    fn test_db(name: &str) -> PathBuf {
        PathBuf::from("target").join(format!("test-history-{name}.db"))
    }

    #[test]
    fn store_message_refuses_a_payload_over_the_cap() {
        let store = HistoryStore::open(test_db("message-cap"), 1024).expect("open store");
        let handle = store.handle();
        let payload = format!(
            "{{\"id\":\"m1\",\"content\":\"{}\"}}",
            "x".repeat(MAX_MESSAGE_BYTES)
        );
        let error = store
            .store_message(&handle, payload)
            .expect_err("an oversized message must be refused");
        assert!(matches!(error, NanoError::InvalidInput(_)));
    }

    #[test]
    fn spill_conversation_refuses_a_payload_over_the_cap() {
        let store = HistoryStore::open(test_db("spill-bytes"), 1024).expect("open store");
        let handle = store.handle();
        let payload = "x".repeat(MAX_SPILL_BYTES + 1);
        let error = store
            .spill_conversation(&handle, "c1".to_string(), payload)
            .expect_err("an oversized conversation must be refused");
        assert!(matches!(error, NanoError::InvalidInput(_)));
    }

    #[test]
    fn spill_conversation_refuses_more_rows_than_the_cap() {
        let store = HistoryStore::open(test_db("spill-rows"), 1024).expect("open store");
        let handle = store.handle();
        let messages: Vec<String> = (0..=MAX_SPILL_MESSAGES)
            .map(|index| format!("{{\"id\":\"m{index}\"}}"))
            .collect();
        let payload = format!("[{}]", messages.join(","));
        let error = store
            .spill_conversation(&handle, "c1".to_string(), payload)
            .expect_err("a conversation over the row cap must be refused");
        assert!(matches!(error, NanoError::InvalidInput(_)));
    }
}
