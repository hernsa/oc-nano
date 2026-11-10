//! oc-nano-compaction — streaming single-pass bounded compaction.
//!
//! Key design fix (from audit): fixed target size is computed **once up
//! front** and there is **exactly one shrink pass** over the messages — no
//! `while(true)` loop that rebuilds the output up to 4x. The result is
//! bounded and predictable.

use oc_nano_core::{MsgRole, NanoMessage};

/// Fixed overhead reserved for framing, separators, and slop.
pub const COMPACTION_OVERHEAD: usize = 256;

/// Estimated serialized size of the single summary system message.
pub const SUMMARY_EST: usize = 320;

/// Per-message wrapper/slack allowance in the budget.
pub const WRAPPER: usize = 8;

/// Serialized size of a single message (its JSON form).
pub fn message_size(msg: &NanoMessage) -> usize {
    serde_json::to_string(msg).map(|s| s.len()).unwrap_or(0)
}

/// Serialize a message slice to a JSON array string.
fn serialize(messages: &[NanoMessage]) -> String {
    serde_json::to_string(messages).unwrap_or_else(|_| "[]".to_string())
}

/// Full serialized size of a slice, including array brackets and commas.
fn serialized_size(messages: &[NanoMessage]) -> usize {
    let mut size = 2usize; // `[` `]`
    for (i, m) in messages.iter().enumerate() {
        if i > 0 {
            size += 1; // comma separator
        }
        size += message_size(m);
    }
    size
}

/// Walk backward from the end of `messages`, starting at `start`, and find
/// the leftmost index of a recent suffix whose cumulative (size + WRAPPER)
/// stays within `budget`. Returns that start index (inclusive).
fn recent_suffix(messages: &[NanoMessage], start: usize, budget: usize) -> usize {
    let mut idx = messages.len();
    let mut used = 0usize;
    while idx > start {
        let sz = message_size(&messages[idx - 1]).saturating_add(WRAPPER);
        if used.saturating_add(sz) > budget {
            break;
        }
        used = used.saturating_add(sz);
        idx -= 1;
    }
    idx
}

/// Build one summary system message carrying role counts for dropped msgs.
///
/// The summary is synthetic (not stored), so it carries sentinel ids and no
/// attachments.
fn summarize_dropped(dropped: &[NanoMessage]) -> NanoMessage {
    let mut system = 0usize;
    let mut user = 0usize;
    let mut assistant = 0usize;
    let mut tool = 0usize;
    for m in dropped {
        match m.role {
            MsgRole::System => system += 1,
            MsgRole::User => user += 1,
            MsgRole::Assistant => assistant += 1,
            MsgRole::Tool => tool += 1,
        }
    }
    let content = format!(
        "[summary] dropped {} messages: system={}, user={}, assistant={}, tool={}",
        dropped.len(),
        system,
        user,
        assistant,
        tool
    );
    NanoMessage {
        id: 0,
        conversation_id: 0,
        role: MsgRole::System,
        content,
        attachment_ids: Vec::new(),
    }
}

/// Single shared compaction pass. `emit_summary` controls whether dropped
/// older messages are collapsed into one summary system message
/// (`compact_messages`) or simply discarded (`compact_prune_messages`).
///
/// Single pass, bounded: target is fixed up front, one backward walk to
/// select the suffix, one forward build of the output. No loops that
/// rebuild.
fn compact_inner(
    messages: &[NanoMessage],
    target_bytes: i64,
    emit_summary: bool,
) -> String {
    let target = target_bytes.max(0) as usize;

    // Fast path: already within budget — return verbatim, no work.
    if serialized_size(messages) <= target {
        return serialize(messages);
    }

    let mut out: Vec<NanoMessage> = Vec::new();
    let mut body_start = 0usize;

    // Preserve a leading system message if present.
    if let Some(first) = messages.first() {
        if first.role == MsgRole::System {
            out.push(first.clone());
            body_start = 1;
        }
    }

    // Fixed budget for the suffix (+ optional summary), computed once.
    let mut budget = target.saturating_sub(COMPACTION_OVERHEAD);
    if !out.is_empty() {
        budget = budget.saturating_sub(message_size(&out[0]).saturating_add(WRAPPER));
    }
    if emit_summary && body_start < messages.len() {
        budget = budget.saturating_sub(SUMMARY_EST);
    }

    // One backward walk: select the recent suffix that fits the budget.
    let suffix_start = recent_suffix(messages, body_start, budget);
    let dropped = &messages[body_start..suffix_start];

    if emit_summary && !dropped.is_empty() {
        out.push(summarize_dropped(dropped));
    }
    for msg in &messages[suffix_start..] {
        out.push(msg.clone());
    }

    serialize(&out)
}

/// Compact `messages` to fit within `target_bytes` bytes.
///
/// Single pass: older messages are summarized into **one** summary system
/// message, the recent suffix is kept verbatim, and a leading system
/// message is preserved. Fixed target size up front; exactly one shrink
/// pass; no `while(true)` loop.
pub fn compact_messages(messages: &[NanoMessage], target_bytes: i64) -> String {
    compact_inner(messages, target_bytes, true)
}

/// Compact `messages` to fit within `target_bytes` bytes by pruning.
///
/// Older messages are dropped entirely (no summary emitted). Keeps the
/// recent suffix verbatim plus the optional leading system message.
pub fn compact_prune_messages(messages: &[NanoMessage], target_bytes: i64) -> String {
    compact_inner(messages, target_bytes, false)
}
