use oc_nano_compaction::{compact_messages, compact_prune_messages, COMPACTION_OVERHEAD};
use oc_nano_core::{MsgRole, NanoMessage};

fn msg(role: MsgRole, content: &str) -> NanoMessage {
    NanoMessage {
        id: 1,
        conversation_id: 1,
        role,
        content: content.to_string(),
        attachment_ids: Vec::new(),
    }
}

#[test]
fn test_compaction_single_pass_bounded() {
    let msgs = vec![msg(MsgRole::User, "hello")];
    let result = compact_messages(&msgs, 1000);
    assert!(result.len() <= 1000 + COMPACTION_OVERHEAD);
    // Fits verbatim — no summary, no pruning.
    assert_eq!(
        result,
        r#"[{"id":1,"conversation_id":1,"role":"user","content":"hello","attachment_ids":[]}]"#
    );
}

#[test]
fn test_compaction_emits_single_summary() {
    let msgs: Vec<NanoMessage> = (0..50).map(|i| msg(MsgRole::User, &format!("message {i}"))).collect();
    let result = compact_messages(&msgs, 200);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    // Exactly one summary system message leads the output.
    assert_eq!(parsed[0]["role"], "system");
    assert!(parsed[0]["content"].as_str().unwrap().contains("dropped"));
    assert_eq!(parsed[0]["content"].as_str().unwrap().matches("system").count(), 1);
    // Tail kept verbatim.
    assert_eq!(parsed[parsed.len() - 1]["role"], "user");
}

#[test]
fn test_compact_prune_emits_no_summary() {
    let msgs: Vec<NanoMessage> = (0..50).map(|i| msg(MsgRole::User, &format!("message {i}"))).collect();
    let result = compact_prune_messages(&msgs, 200);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    // Pruning drops older messages entirely — no system summary inserted.
    assert!(parsed.iter().all(|v| v["role"] == "user"));
    assert!(!parsed.iter().any(|v| v["content"].as_str().unwrap_or_default().contains("dropped")));
}

#[test]
fn test_compaction_preserves_leading_system() {
    let mut msgs: Vec<NanoMessage> = vec![msg(MsgRole::System, "system prompt")];
    msgs.extend((0..30).map(|i| msg(MsgRole::User, &format!("message {i}"))));
    let result = compact_messages(&msgs, 150);
    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed[0]["role"], "system");
    assert_eq!(parsed[0]["content"], "system prompt");
}
