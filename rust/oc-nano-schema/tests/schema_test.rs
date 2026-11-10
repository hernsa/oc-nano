use oc_nano_schema::{hash_tool_set, CachedSchema, SchemaCache};
use oc_nano_core::SchemaCacheEntry;
use std::time::Duration;

#[test]
fn test_schema_hash_stable_across_restarts() {
    let h1 = hash_tool_set(r#"[{"name":"a"}]"#);
    let h2 = hash_tool_set(r#"[{"name":"a"}]"#);
    assert_eq!(h1, h2);
    assert_ne!(h1, 0);
}

#[test]
fn test_schema_hash_different_inputs_differ() {
    let h1 = hash_tool_set(r#"[{"name":"a"}]"#);
    let h2 = hash_tool_set(r#"[{"name":"b"}]"#);
    assert_ne!(h1, h2);
}

#[test]
fn test_hash_tool_set_is_order_sensitive() {
    let h1 = hash_tool_set(r#"[{"name":"a"},{"name":"b"}]"#);
    let h2 = hash_tool_set(r#"[{"name":"b"},{"name":"a"}]"#);
    assert_ne!(h1, h2);
}

#[test]
fn test_get_put_roundtrip() {
    let cache = SchemaCache::new();
    cache.put("model-a", 42, r#"{"type":"object"}"#.to_string(), 60);
    assert_eq!(
        cache.get("model-a", 42),
        Some(r#"{"type":"object"}"#.to_string())
    );
    assert!(cache.get("model-b", 42).is_none());
    assert!(cache.get("model-a", 99).is_none());
    assert!(cache.get("missing", 0).is_none());
}

#[test]
fn test_expired_entry_returns_none_and_is_removed() {
    let cache = SchemaCache::new();
    cache.put("model-a", 42, r#"{"type":"object"}"#.to_string(), 0);
    assert!(cache.get("model-a", 42).is_none());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_evict_expired_removes_expired_only() {
    let cache = SchemaCache::new();
    cache.put("a", 1, "{}".to_string(), 60);
    cache.put("b", 2, "{}".to_string(), 0);
    cache.put("c", 3, "{}".to_string(), 0);
    assert_eq!(cache.len(), 3);

    let removed = cache.evict_expired();
    assert_eq!(removed, 2);
    assert_eq!(cache.len(), 1);
    assert_eq!(cache.get("a", 1), Some("{}".to_string()));
}

#[test]
fn test_len_is_empty_clear_and_default() {
    let cache = SchemaCache::new();
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);

    let _default = SchemaCache::default();
    assert!(_default.is_empty());

    cache.put("a", 1, "{}".to_string(), 60);
    assert!(!cache.is_empty());
    assert_eq!(cache.len(), 1);

    cache.clear();
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);
}

#[test]
fn test_from_entry_consumes_core_type() {
    let entry = SchemaCacheEntry {
        model: "model-a".to_string(),
        hash: 42,
        schema_json: r#"{"type":"object"}"#.to_string(),
        inserted_at: 0,
        ttl_secs: 60,
    };
    let cached = CachedSchema::from_entry(entry);
    assert_eq!(cached.schema_json, r#"{"type":"object"}"#);
    assert_eq!(cached.ttl, Duration::from_secs(60));
}
