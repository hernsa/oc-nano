use oc_nano_core::*;

#[test]
fn test_constants_isolation() {
    assert_eq!(BINARY_NAME, "oc-nano");
    assert_ne!(BINARY_NAME, "opencode");
    assert_ne!(BINARY_NAME, "opencode-lean");
    assert_eq!(CONFIG_DIR_NAME, "oc-nano");
    assert_eq!(DATA_DIR_NAME, "oc-nano");
    assert_eq!(HISTORY_DB_NAME, "oc-nano-history.db");
    assert_ne!(HISTORY_DB_NAME, "history.sqlite");
    assert_eq!(DEFAULT_RAM_MESSAGE_CAP, 2000);
    assert_eq!(DEFAULT_STATE_TTL_SECS, 300);
}

#[test]
fn test_nano_id_new_and_nil_differ() {
    let id = NanoId::new();
    assert_ne!(id, NanoId::nil());
    // round-trips through Display / serde
    let _ = id.to_string();
    let json = serde_json::to_string(&id).unwrap();
    let back: NanoId = serde_json::from_str(&json).unwrap();
    assert_eq!(id, back);
}

#[test]
fn test_role_enum_serialization() {
    for role in [Role::System, Role::User, Role::Assistant, Role::Tool] {
        let json = serde_json::to_string(&role).unwrap();
        let back: Role = serde_json::from_str(&json).unwrap();
        assert_eq!(role, back);
    }
}

#[test]
fn test_nano_message_round_trip() {
    let conv = NanoId::new();
    let msg = NanoMessage::new(conv, Role::Assistant, "hello world");
    let json = serde_json::to_string(&msg).unwrap();
    let back: NanoMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg.id, back.id);
    assert_eq!(msg.conversation_id, conv);
    assert_eq!(msg.role, Role::Assistant);
    assert_eq!(msg.content, "hello world");
    assert!(msg.tool_calls.is_empty());
    assert!(msg.tool_results.is_empty());
}

#[test]
fn test_nano_model_config_serialization() {
    let cfg = NanoModelConfig {
        model: "gpt-4o-mini".into(),
        temperature: Some(0.7),
        max_tokens: Some(512),
        top_p: Some(0.9),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: NanoModelConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg, back);
    // PartialEq exists but Eq must NOT (contains Option<f64>).
    let _ = cfg == back;
}

#[test]
fn test_nano_error_variants() {
    let io_err = NanoError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "missing"));
    assert!(io_err.to_string().contains("io error"));
    let not_found = NanoError::NotFound("thing".into());
    assert!(not_found.to_string().contains("not found"));
    let internal = NanoError::Internal("boom".into());
    assert!(internal.to_string().contains("internal error"));
}
