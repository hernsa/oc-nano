//! oc-nano-core — shared primitives, constants, and the error model.
//!
//! All identifiers are renamed to avoid collision with OpenCode
//! (`oc-nano` binary/config/data dir, `oc-nano-history.db`).

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

pub const BINARY_NAME: &str = "oc-nano";
pub const CONFIG_DIR_NAME: &str = "oc-nano";
pub const DATA_DIR_NAME: &str = "oc-nano";
pub const HISTORY_DB_NAME: &str = "oc-nano-history.db";
pub const DEFAULT_RAM_MESSAGE_CAP: usize = 2000;
pub const DEFAULT_STATE_TTL_SECS: u64 = 300;

/// Opaque identifier for any entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NanoId(pub Uuid);

/// Identifier for a conversation / session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

/// Role of a message within a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MsgRole {
    User,
    Assistant,
    System,
    Tool,
}

impl std::fmt::Display for MsgRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MsgRole::User => write!(f, "user"),
            MsgRole::Assistant => write!(f, "assistant"),
            MsgRole::System => write!(f, "system"),
            MsgRole::Tool => write!(f, "tool"),
        }
    }
}

/// Alias for `MsgRole`, used by the prompt arena.
pub type Role = MsgRole;

/// A message in the compaction pipeline.
///
/// Fields mirror [`Message`] but use primitive IDs (`u64`/`i64`) so the type
/// can cross FFI boundaries as JSON without pulling in `Uuid`/`SessionId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NanoMessage {
    pub id: u64,
    pub conversation_id: i64,
    pub role: MsgRole,
    pub content: String,
    pub attachment_ids: Vec<String>,
}

/// A single conversation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub id: NanoId,
    pub conversation_id: SessionId,
    pub role: MsgRole,
    pub content: String,
    pub created_at: String,
}

/// An invocation of a tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub id: NanoId,
    pub name: String,
    pub arguments: String,
}

/// The outcome of a tool invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub invocation_id: NanoId,
    pub output: String,
    pub success: bool,
}

/// LLM model settings. Deliberately does NOT derive `Eq` (contains `Option<f64>`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSettings {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
}

/// Cached tool schema entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaCacheEntry {
    pub model: String,
    pub hash: u64,
    pub schema_json: String,
    pub ttl_secs: u64,
    pub inserted_at: u64,
}

/// Error type for oc-nano.
#[derive(Debug, Error)]
pub enum NanoError {
    #[error("store is closed")]
    Closed,
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("{0}")]
    Other(String),
}

/// Convenience result alias.
pub type NanoResult<T> = Result<T, NanoError>;
