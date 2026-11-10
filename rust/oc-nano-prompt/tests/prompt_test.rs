use oc_nano_core::{MsgRole, Role};
use oc_nano_prompt::PromptArena;

#[test]
fn test_prompt_arena_arc_str_no_clone() {
    let arena = PromptArena::new();
    let id = arena.add_message("hello".to_string());
    let c1 = arena.get_message(id).unwrap();
    let c2 = arena.get_message(id).unwrap();
    // Same Arc<str> allocation — no per-materialization clone.
    assert!(std::ptr::eq(&*c1, &*c2));
}

#[test]
fn test_add_message_to_conversation_role() {
    let arena = PromptArena::new();
    let id = arena.add_message_to_conversation(7, Role::Assistant, "hi".to_string());
    let content = arena.get_message(id).unwrap();
    assert_eq!(&*content, "hi");
    let _ = arena.get_message(id).unwrap();
    assert_eq!(arena.enforce_ram_cap(7), 0);
}

#[test]
fn test_register_conversation_handle() {
    let arena = PromptArena::new();
    let id = oc_nano_core::NanoId::new();
    let h1 = arena.register_conversation(id);
    let h2 = arena.register_conversation(id); // same id -> same handle
    assert_eq!(h1, h2);
    assert_ne!(h1, 0);
}

#[test]
fn test_store_message_from_nano_message() {
    let arena = PromptArena::new();
    let nano = oc_nano_core::NanoMessage {
        id: 999, // ignored; arena assigns fresh id
        conversation_id: 3,
        role: MsgRole::Tool,
        content: "tool output".to_string(),
        attachment_ids: vec!["att-a".to_string()],
    };
    let id = arena.store_message(&nano);
    assert_ne!(id, 999);
    let content = arena.get_message(id).unwrap();
    assert_eq!(&*content, "tool output");
    assert_eq!(arena.materialize(3).lines().next().unwrap(), "[tool] tool output");
}

#[test]
fn test_raw_attachments_no_base64() {
    let arena = PromptArena::new();
    let bytes = b"raw attachment bytes".to_vec();
    arena.add_attachment("att-1".to_string(), bytes.clone());
    let got = arena.get_attachment("att-1").unwrap();
    assert_eq!(got, b"raw attachment bytes");
    assert_eq!(got, bytes);
    assert!(arena.get_attachment("missing").is_none());
}

#[test]
fn test_materialize_format() {
    let arena = PromptArena::new();
    let _id1 = arena.add_message_to_conversation(5, MsgRole::User, "ping".to_string());
    arena.add_attachment("att-x".to_string(), vec![1, 2, 3]);
    // attach the attachment id to the message manually via store path is not
    // exposed, so just check the base format line.
    let out = arena.materialize(5);
    assert!(out.starts_with("[user] ping"));
    assert!(!out.contains("[attachment]"));
    let empty = arena.materialize(999);
    assert!(empty.is_empty());
}
