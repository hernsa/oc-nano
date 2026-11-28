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
fn test_nano_id_wraps_a_uuid_and_round_trips() {
    let id = NanoId(uuid::Uuid::new_v4());
    // Two fresh ids never collide.
    assert_ne!(id, NanoId(uuid::Uuid::new_v4()));
    // Round-trips through serde.
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
    // The role names are lowercased on the wire.
    assert_eq!(serde_json::to_string(&Role::User).unwrap(), "\"user\"");
    assert_eq!(Role::Assistant.to_string(), "assistant");
}

#[test]
fn test_nano_message_round_trip() {
    let msg = NanoMessage {
        id: 7,
        conversation_id: 42,
        role: Role::Assistant,
        content: "hello world".to_string(),
        attachment_ids: vec!["att-1".to_string()],
    };
    let json = serde_json::to_string(&msg).unwrap();
    let back: NanoMessage = serde_json::from_str(&json).unwrap();
    assert_eq!(msg, back);
    assert_eq!(msg.id, 7);
    assert_eq!(msg.conversation_id, 42);
    assert_eq!(msg.role, Role::Assistant);
    assert_eq!(msg.content, "hello world");
    assert_eq!(msg.attachment_ids, vec!["att-1".to_string()]);
}

#[test]
fn test_model_settings_serialization() {
    let cfg = ModelSettings {
        temperature: Some(0.7),
        max_tokens: Some(512),
        top_p: Some(0.9),
    };
    let json = serde_json::to_string(&cfg).unwrap();
    let back: ModelSettings = serde_json::from_str(&json).unwrap();
    assert_eq!(cfg, back);
    // PartialEq exists but Eq must NOT (contains Option<f64>).
    let _ = cfg == back;
}

#[test]
fn test_nano_error_variants() {
    let closed = NanoError::Closed;
    assert!(closed.to_string().contains("store is closed"));
    let invalid = NanoError::InvalidInput("bad payload".into());
    assert!(invalid.to_string().contains("invalid input"));
    assert!(invalid.to_string().contains("bad payload"));
    let other = NanoError::Other("boom".into());
    assert!(other.to_string().contains("boom"));
}
