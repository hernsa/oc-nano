//! oc-nano-core — renamed types and constants for the oc-nano hot-path rewrite.
//!
//! Every identifier here is deliberately renamed to avoid collision with
//! OpenCode (`opencode`, `opencode-lean`). This is the foundation crate that
//! every other oc-nano crate consumes.

use std::fmt;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Constants (renamed, isolated from OpenCode)
// ---------------------------------------------------------------------------

/// Binary name — never `opencode` or `opencode-lean`.
pub const BINARY_NAME: &str = "oc-nano";

/// Config directory name — never `~/.config/opencode` or `~/.config/opencode-lean`.
pub const CONFIG_DIR_NAME: &str = "oc-nano";

/// Data directory name — never `~/.local/share/opencode` or `~/.local/share/opencode-lean`.
pub const DATA_DIR_NAME: &str = "oc-nano";

/// History database file name — never `history.sqlite`.
pub const HISTORY_DB_NAME: &str = "oc-nano-history.db";

/// Default RAM message cap — enforced, not just a constant.
pub const DEFAULT_RAM_MESSAGE_CAP: usize = 2000;

/// Default state TTL in seconds.
pub const DEFAULT_STATE_TTL_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// NanoId — opaque, collision-free identifier
// ---------------------------------------------------------------------------

/// Opaque identifier for conversations/messages, built on `uuid::Uuid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NanoId(pub Uuid);

impl NanoId {
    /// Generate a random v4 `NanoId`.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for NanoId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for NanoId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

// ---------------------------------------------------------------------------
// SessionId
// ---------------------------------------------------------------------------

/// Opaque identifier for a chat session.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub Uuid);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for SessionId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Self)
    }
}

// ---------------------------------------------------------------------------
// Role / MsgRole
// ---------------------------------------------------------------------------

/// Role of a message sender.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MsgRole {
    #[default]
    User,
    Assistant,
    System,
    Tool,
}

/// Alias used by the prompt layer.
pub type Role = MsgRole;

impl fmt::Display for MsgRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MsgRole::User => write!(f, "user"),
            MsgRole::Assistant => write!(f, "assistant"),
            MsgRole::System => write!(f, "system"),
            MsgRole::Tool => write!(f, "tool"),
        }
    }
}

// ---------------------------------------------------------------------------
// Message / NanoMessage
// ---------------------------------------------------------------------------

/// A message transported between layers.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    pub id: u64,
    pub conversation_id: i64,
    pub role: MsgRole,
    pub content: String,
    pub attachment_ids: Vec<String>,
}

/// Alias used by the prompt layer.
pub type NanoMessage = Message;

// ---------------------------------------------------------------------------
// ToolInvocation / ToolOutcome
// ---------------------------------------------------------------------------

/// A tool call invocation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolInvocation {
    pub call_id: String,
    pub name: String,
    pub arguments: String,
}

/// A tool call outcome.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ToolOutcome {
    pub call_id: String,
    pub output: String,
}

// ---------------------------------------------------------------------------
// ModelSettings — deliberately does NOT derive `Eq` (has `Option<f64>`)
// ---------------------------------------------------------------------------

/// Model sampling settings.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ModelSettings {
    pub temperature: Option<f64>,
    pub top_p: Option<f64>,
    pub max_tokens: Option<u32>,
}

// ---------------------------------------------------------------------------
// SchemaCacheEntry
// ---------------------------------------------------------------------------

/// Cached schema entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SchemaCacheEntry {
    pub model: String,
    pub schema_json: String,
    pub inserted_at: u64,
    pub ttl_secs: u64,
}

// ---------------------------------------------------------------------------
// Error model
// ---------------------------------------------------------------------------

/// oc-nano error type.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum NanoError {
    NotFound,
    AlreadyExists,
    InvalidInput,
    Io(String),
    Closed,
    Other(String),
}

impl fmt::Display for NanoError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NanoError::NotFound => write!(f, "not found"),
            NanoError::AlreadyExists => write!(f, "already exists"),
            NanoError::InvalidInput => write!(f, "invalid input"),
            NanoError::Io(msg) => write!(f, "io error: {msg}"),
            NanoError::Closed => write!(f, "handle closed"),
            NanoError::Other(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for NanoError {}

/// Result alias used throughout oc-nano.
pub type NanoResult<T> = Result<T, NanoError>;
