//! oc-nano-prompt — arena-backed message storage with raw attachment bytes.
//!
//! Design notes (the audit fixes applied here):
//! * Message content is stored as `Arc<str>` so that `get_message` hands out
//!   the same allocation every time — materialization never clones strings.
//! * Attachments are stored as raw `Vec<u8>` — no base64 encode/decode
//!   round-trip anywhere in the hot path.

use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use lru::LruCache;
use oc_nano_core::{DEFAULT_RAM_MESSAGE_CAP, NanoId, NanoMessage, Role};

pub const ATTACHMENT_BYTE_BUDGET: usize = 8 * 1024 * 1024;
const ATTACHMENT_ENTRY_LIMIT: usize = 128;

struct AttachmentCache {
    entries: LruCache<String, Vec<u8>>,
    bytes: usize,
}

// ---------------------------------------------------------------------------
// Stored message
// ---------------------------------------------------------------------------

/// A message stored in the arena. Content is an `Arc<str>` so that many
/// readers can share the same allocation without cloning the underlying
/// string data.
#[derive(Clone, Debug)]
pub struct StoredMessage {
    pub id: u64,
    pub conversation_id: i64,
    pub role: Role,
    pub content: Arc<str>,
    pub attachment_ids: Vec<String>,
}

// ---------------------------------------------------------------------------
// PromptArena
// ---------------------------------------------------------------------------

/// Arena-backed in-memory message store.
///
/// * `messages` — message id → stored message
/// * `conversations` — conversation id → ordered list of message ids
/// * `conversation_handles` — external `NanoId` → internal conversation id
/// * `attachments` — attachment id → raw bytes (no base64)
/// * `next_id` — monotonically increasing message id counter
/// * `next_handle` — monotonically increasing conversation handle counter
pub struct PromptArena {
    pub messages: DashMap<u64, StoredMessage>,
    pub conversations: DashMap<i64, Vec<u64>>,
    pub conversation_handles: DashMap<NanoId, i64>,
    attachments: Mutex<AttachmentCache>,
    pub next_id: AtomicU64,
    pub next_handle: AtomicI64,
}

impl PromptArena {
    /// Create a new, empty arena.
    pub fn new() -> Self {
        Self {
            messages: DashMap::new(),
            conversations: DashMap::new(),
            conversation_handles: DashMap::new(),
            attachments: Mutex::new(AttachmentCache {
                entries: LruCache::new(NonZeroUsize::new(ATTACHMENT_ENTRY_LIMIT).unwrap()),
                bytes: 0,
            }),
            next_id: AtomicU64::new(1),
            next_handle: AtomicI64::new(1),
        }
    }

    /// Alias for `new()`, matching the napi binding name `prompt_init`.
    pub fn init() -> Self {
        Self::new()
    }

    /// Add a message to conversation `0` with role `User`.
    ///
    /// Returns the newly assigned message id.
    pub fn add_message(&self, content: String) -> u64 {
        self.add_message_to_conversation(0, Role::User, content)
    }

    /// Add a message to an explicit conversation.
    ///
    /// Returns the newly assigned message id.
    pub fn add_message_to_conversation(
        &self,
        conversation_id: i64,
        role: Role,
        content: String,
    ) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = StoredMessage {
            id,
            conversation_id,
            role,
            content: Arc::from(content),
            attachment_ids: Vec::new(),
        };
        self.messages.insert(id, message);
        self.conversations
            .entry(conversation_id)
            .or_default()
            .push(id);
        id
    }

    /// Register an external `NanoId` and return the internal conversation
    /// handle.
    ///
    /// If the `NanoId` is already registered, its existing handle is returned.
    /// Otherwise a fresh handle is allocated and an empty conversation slot is
    /// reserved.
    pub fn register_conversation(&self, id: NanoId) -> i64 {
        if let Some(handle) = self.conversation_handles.get(&id) {
            return *handle;
        }
        let handle = self.next_handle.fetch_add(1, Ordering::Relaxed);
        self.conversation_handles.insert(id, handle);
        self.conversations.entry(handle).or_default();
        handle
    }

    /// Store a `NanoMessage` in the arena, assigning it a fresh arena id.
    ///
    /// Returns the newly assigned message id.
    pub fn store_message(&self, message: &NanoMessage) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let stored = StoredMessage {
            id,
            conversation_id: message.conversation_id,
            role: message.role,
            content: Arc::from(message.content.clone()),
            attachment_ids: message.attachment_ids.clone(),
        };
        self.messages.insert(id, stored);
        self.conversations
            .entry(message.conversation_id)
            .or_default()
            .push(id);
        id
    }

    /// Get a message's content as an `Arc<str>`.
    ///
    /// The returned `Arc` shares the same allocation as the stored message —
    /// no string data is cloned per materialization.
    pub fn get_message(&self, id: u64) -> Option<Arc<str>> {
        self.messages.get(&id).map(|entry| entry.content.clone())
    }

    /// Store raw attachment bytes under `id` (no base64).
    pub fn add_attachment(&self, id: String, bytes: Vec<u8>) {
        let mut cache = self.attachments.lock().unwrap();
        if let Some(previous) = cache.entries.pop(&id) {
            cache.bytes -= previous.len();
        }
        if bytes.len() > ATTACHMENT_BYTE_BUDGET {
            return;
        }
        while cache.bytes + bytes.len() > ATTACHMENT_BYTE_BUDGET
            || cache.entries.len() >= ATTACHMENT_ENTRY_LIMIT
        {
            if let Some((_, evicted)) = cache.entries.pop_lru() {
                cache.bytes -= evicted.len();
            }
        }
        cache.bytes += bytes.len();
        cache.entries.put(id, bytes);
    }

    /// Get raw attachment bytes by id (no base64 re-encode).
    pub fn get_attachment(&self, id: &str) -> Option<Vec<u8>> {
        self.attachments.lock().unwrap().entries.get(id).cloned()
    }

    /// Materialize a conversation into a single string.
    ///
    /// Each message is rendered as `[role] content` followed by one
    /// `[attachment] <id>` line per attached attachment.
    pub fn materialize(&self, conversation_id: i64) -> String {
        let ids = match self.conversations.get(&conversation_id) {
            Some(ids) => ids.clone(),
            None => return String::new(),
        };

        let mut out = String::new();
        for (index, id) in ids.iter().enumerate() {
            let Some(message) = self.messages.get(id) else {
                continue;
            };
            if index > 0 {
                out.push('\n');
            }
            out.push('[');
            out.push_str(message.role.to_string().as_str());
            out.push_str("] ");
            out.push_str(&*message.content);
            for attachment_id in &message.attachment_ids {
                out.push('\n');
                out.push_str("[attachment] ");
                out.push_str(attachment_id);
            }
        }
        out
    }

    /// Evict messages from `conversation_handle` when the count exceeds `DEFAULT_RAM_MESSAGE_CAP`.
    ///
    /// Evicts the oldest messages first (ids are monotonically increasing, so the front of the
    /// per-conversation id vec is the oldest). Returns how many messages were evicted.
    pub fn enforce_ram_cap(&self, conversation_handle: i64) -> i64 {
        let over = self
            .conversations
            .get(&conversation_handle)
            .map(|ids| ids.len().saturating_sub(DEFAULT_RAM_MESSAGE_CAP))
            .unwrap_or(0);
        if over == 0 {
            return 0;
        }

        let mut evicted = 0;
        if let Some(mut ids) = self.conversations.get_mut(&conversation_handle) {
            let to_evict = over.min(ids.len());
            // ids are pushed to the end, so the front is the oldest. Drain from the front.
            let evict_ids: Vec<u64> = ids.drain(..to_evict).collect();
            for id in evict_ids {
                self.messages.remove(&id);
                evicted += 1;
            }
        }
        evicted as i64
    }
}

impl Default for PromptArena {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachments_evict_least_recently_used_within_byte_budget() {
        let arena = PromptArena::new();
        arena.add_attachment("first".into(), vec![1; 4 * 1024 * 1024]);
        arena.add_attachment("second".into(), vec![2; 4 * 1024 * 1024]);
        assert!(arena.get_attachment("first").is_some());
        arena.add_attachment("third".into(), vec![3; 4 * 1024 * 1024]);
        assert!(arena.get_attachment("second").is_none());
        assert!(arena.get_attachment("first").is_some());
        assert!(arena.get_attachment("third").is_some());
    }

    #[test]
    fn oversized_attachments_are_not_retained() {
        let arena = PromptArena::new();
        arena.add_attachment("large".into(), vec![0; 8 * 1024 * 1024 + 1]);
        assert!(arena.get_attachment("large").is_none());
    }

    #[test]
    fn attachments_are_entry_bounded_even_when_empty() {
        let arena = PromptArena::new();
        for id in 0..129 {
            arena.add_attachment(id.to_string(), vec![]);
        }
        assert!(arena.get_attachment("0").is_none());
        assert!(arena.get_attachment("128").is_some());
    }

    #[test]
    fn replacement_does_not_double_count_bytes() {
        let arena = PromptArena::new();
        arena.add_attachment("first".into(), vec![1; 4 * 1024 * 1024]);
        arena.add_attachment("first".into(), vec![2; 4 * 1024 * 1024]);
        arena.add_attachment("second".into(), vec![3; 4 * 1024 * 1024]);
        assert_eq!(arena.get_attachment("first").unwrap()[0], 2);
        assert!(arena.get_attachment("second").is_some());
    }
}
