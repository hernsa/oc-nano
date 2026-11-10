use oc_nano_history::{HistoryStore, Handle};

#[test]
fn test_history_open_close_no_leak() {
    let store = HistoryStore::open(":memory:", 8192).unwrap();
    let handle: Handle = store.handle();
    HistoryStore::close(&handle).unwrap();
    // after close, store_message should error (connection dropped)
    assert!(store.store_message(&handle, "{}").is_err());
}

#[test]
fn test_history_store_and_load_streaming() {
    let store = HistoryStore::open(":memory:", 8192).unwrap();
    let handle = store.handle();
    let msg = r#"{"id":"11111111-1111-1111-1111-111111111111","conversation_id":"conv-1","role":"user","content":"hello world","created_at":"2026-11-10T00:00:00+00:00"}"#;
    assert!(store.store_message(&handle, msg.to_string()).is_ok());
    let loaded = store.load_conversation_json(&handle, "conv-1".to_string()).unwrap();
    assert!(loaded.contains("hello world"));
    assert!(loaded.contains("\"role\":\"user\""));
    // streaming output is a JSON array
    assert!(loaded.starts_with('['));
    assert!(loaded.ends_with(']'));
}

#[test]
fn test_history_spill_conversation() {
    let store = HistoryStore::open(":memory:", 8192).unwrap();
    let handle = store.handle();
    let messages = r#"[
        {"id":"22222222-2222-2222-2222-222222222222","conversation_id":"conv-2","role":"assistant","content":"hi there","created_at":"2026-11-10T00:00:01+00:00"},
        {"id":"22222222-2222-2222-2222-222222222223","conversation_id":"conv-2","role":"tool","content":"ok","created_at":"2026-11-10T00:00:02+00:00"}
    ]"#;
    let count = store
        .spill_conversation(&handle, "conv-2".to_string(), messages.to_string())
        .unwrap();
    assert_eq!(count, 2);
    let loaded = store.load_conversation_json(&handle, "conv-2".to_string()).unwrap();
    assert_eq!(loaded.matches("hi there").count(), 1);
    assert_eq!(loaded.matches("ok").count(), 1);
}

#[test]
fn test_history_prune_before_no_vacuum_every_call() {
    let store = HistoryStore::open(":memory:", 8192).unwrap();
    let handle = store.handle();
    let count = store
        .prune_before(&handle, "2000-01-01T00:00:00+00:00".to_string())
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn test_history_prune_old_vacuums() {
    let store = HistoryStore::open(":memory:", 8192).unwrap();
    let handle = store.handle();
    // ttl of 0 makes everything older than now eligible; empty DB -> 0 deleted
    let count = store.prune_old(&handle, 0).unwrap();
    assert_eq!(count, 0);
}
