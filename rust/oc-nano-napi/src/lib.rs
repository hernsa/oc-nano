//! oc-nano-napi — napi-rs bindings for oc-nano.
//!
//! Every binding is wrapped in `wrap_napi()` so panics are caught and turned
//! into graceful `napi::Error`s instead of aborting the host process (the
//! release profile uses `panic = "abort"`).
//!
//! All numeric FFI types are `i64` (napi-rs does not support `u64`/`usize` as
//! #[napi] types). `conversation_id` crosses FFI as `i64` (internal handle),
//! not as `String`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::sync::atomic::{AtomicI64, Ordering};

use oc_nano_core::{
    BINARY_NAME, CONFIG_DIR_NAME, DATA_DIR_NAME, MsgRole, NanoMessage,
};
use oc_nano_history::HistoryStore;
use oc_nano_schema::SchemaCache;
use oc_nano_state::StatePool;
use oc_nano_prompt::PromptArena;
use oc_nano_compaction::{compact_messages, compact_prune_messages};
use oc_nano_instructions::InstructionsStore;

// ---------------------------------------------------------------------------
// Panic guard — catch_unwind -> napi::Error::from_reason("internal panic")
// ---------------------------------------------------------------------------

/// Run closure `f` under `catch_unwind`. On panic, return a graceful
/// `napi::Error::from_reason("internal panic")` instead of aborting the host.
///
/// The closure must return `napi::Result<R>` so that `?` propagation works
/// naturally inside the binding body.
pub fn wrap_napi<F, R>(f: F) -> napi::Result<R>
where
    F: FnOnce() -> R,
{
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(result) => Ok(result),
        Err(_panic) => Err(napi::Error::from_reason("internal panic")),
    }
}

// ---------------------------------------------------------------------------
// Singletons
// ---------------------------------------------------------------------------

/// Global registry of open `HistoryStore` instances.
struct HistoryHandleRegistry {
    handles: Mutex<HashMap<i64, Arc<HistoryStore>>>,
    counter: AtomicI64,
}

static HISTORY_HANDLES: OnceLock<HistoryHandleRegistry> = OnceLock::new();

fn history_registry() -> &'static HistoryHandleRegistry {
    HISTORY_HANDLES.get_or_init(|| HistoryHandleRegistry {
        handles: Mutex::new(HashMap::new()),
        counter: AtomicI64::new(1),
    })
}

/// Global tool-schema cache (FxHash-backed, cross-process stable).
static SCHEMA_CACHE: OnceLock<SchemaCache> = OnceLock::new();

fn schema_cache() -> &'static SchemaCache {
    SCHEMA_CACHE.get_or_init(SchemaCache::new)
}

/// Global registry of `StatePool` instances.
struct StatePoolRegistry {
    pools: Mutex<HashMap<i64, StatePool>>,
    counter: AtomicI64,
}

static STATE_POOLS: OnceLock<StatePoolRegistry> = OnceLock::new();

fn state_pool_registry() -> &'static StatePoolRegistry {
    STATE_POOLS.get_or_init(|| StatePoolRegistry {
        pools: Mutex::new(HashMap::new()),
        counter: AtomicI64::new(1),
    })
}

/// Global prompt arena.
static PROMPT_ARENA: OnceLock<PromptArena> = OnceLock::new();

fn prompt_arena() -> &'static PromptArena {
    PROMPT_ARENA.get_or_init(PromptArena::new)
}

/// Global instructions store.
static INSTRUCTIONS: OnceLock<InstructionsStore> = OnceLock::new();

fn instructions_store() -> &'static InstructionsStore {
    INSTRUCTIONS.get_or_init(InstructionsStore::new)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn parse_role(s: &str) -> napi::Result<MsgRole> {
    match s {
        "user" => Ok(MsgRole::User),
        "assistant" => Ok(MsgRole::Assistant),
        "system" => Ok(MsgRole::System),
        "tool" => Ok(MsgRole::Tool),
        _ => Err(napi::Error::from_reason("invalid role")),
    }
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

#[napi_derive::napi]
fn binary_name() -> napi::Result<String> {
    wrap_napi(|| Ok(BINARY_NAME.to_string()))?
}

#[napi_derive::napi]
fn config_dir_name() -> napi::Result<String> {
    wrap_napi(|| Ok(CONFIG_DIR_NAME.to_string()))?
}

#[napi_derive::napi]
fn data_dir_name() -> napi::Result<String> {
    wrap_napi(|| Ok(DATA_DIR_NAME.to_string()))?
}

// ---------------------------------------------------------------------------
// History
// ---------------------------------------------------------------------------

#[napi_derive::napi]
fn history_init(db_path: String, cache_size_kb: i64) -> napi::Result<i64> {
    wrap_napi(|| {
        let store = HistoryStore::open(&db_path, cache_size_kb).map_err(|e| {
            napi::Error::from_reason(e.to_string())
        })?;
        let id = history_registry().counter.fetch_add(1, Ordering::Relaxed);
        history_registry()
            .handles
            .lock()
            .unwrap()
            .insert(id, Arc::new(store));
        Ok(id)
    })?
}

#[napi_derive::napi]
fn history_close(handle: i64) -> napi::Result<bool> {
    wrap_napi(|| {
        let registry = history_registry();
        let mut handles = registry.handles.lock().unwrap();
        if let Some(store_arc) = handles.remove(&handle) {
            let h = store_arc.handle();
            HistoryStore::close(&h).map_err(|e| napi::Error::from_reason(e.to_string()))?;
            Ok(true)
        } else {
            Ok(false)
        }
    })?
}

#[napi_derive::napi]
fn history_store_message(handle: i64, message_json: String) -> napi::Result<()> {
    wrap_napi(|| {
        let registry = history_registry();
        let store_arc = {
            let handles = registry.handles.lock().unwrap();
            handles
                .get(&handle)
                .map(Arc::clone)
                .ok_or_else(|| napi::Error::from_reason("invalid history handle"))?
        };
        let h = store_arc.handle();
        store_arc.store_message(&h, message_json).map_err(|e| {
            napi::Error::from_reason(e.to_string())
        })
    })?
}

#[napi_derive::napi]
fn history_load_conversation(handle: i64, conversation_id: String) -> napi::Result<String> {
    wrap_napi(|| {
        let registry = history_registry();
        let store_arc = {
            let handles = registry.handles.lock().unwrap();
            handles
                .get(&handle)
                .map(Arc::clone)
                .ok_or_else(|| napi::Error::from_reason("invalid history handle"))?
        };
        let h = store_arc.handle();
        store_arc
            .load_conversation_json(&h, conversation_id)
            .map_err(|e| napi::Error::from_reason(e.to_string()))
    })?
}

#[napi_derive::napi]
fn history_spill_conversation(
    handle: i64,
    conversation_id: String,
    messages_json: String,
) -> napi::Result<i64> {
    wrap_napi(|| {
        let registry = history_registry();
        let store_arc = {
            let handles = registry.handles.lock().unwrap();
            handles
                .get(&handle)
                .map(Arc::clone)
                .ok_or_else(|| napi::Error::from_reason("invalid history handle"))?
        };
        let h = store_arc.handle();
        let count = store_arc
            .spill_conversation(&h, conversation_id, messages_json)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(count as i64)
    })?
}

#[napi_derive::napi]
fn history_prune_before(handle: i64, cutoff: String) -> napi::Result<i64> {
    wrap_napi(|| {
        let registry = history_registry();
        let store_arc = {
            let handles = registry.handles.lock().unwrap();
            handles
                .get(&handle)
                .map(Arc::clone)
                .ok_or_else(|| napi::Error::from_reason("invalid history handle"))?
        };
        let h = store_arc.handle();
        let count = store_arc
            .prune_before(&h, cutoff)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(count as i64)
    })?
}

#[napi_derive::napi]
fn history_prune_old(handle: i64, ttl_seconds: i64) -> napi::Result<i64> {
    wrap_napi(|| {
        let registry = history_registry();
        let store_arc = {
            let handles = registry.handles.lock().unwrap();
            handles
                .get(&handle)
                .map(Arc::clone)
                .ok_or_else(|| napi::Error::from_reason("invalid history handle"))?
        };
        let h = store_arc.handle();
        let count = store_arc
            .prune_old(&h, ttl_seconds)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(count as i64)
    })?
}

// ---------------------------------------------------------------------------
// Schema cache
// ---------------------------------------------------------------------------

#[napi_derive::napi]
fn schema_cache_hash(tools_json: String) -> napi::Result<i64> {
    wrap_napi(|| {
        let _cache = schema_cache();
        let hash = SchemaCache::hash_tool_set(&tools_json);
        Ok(hash as i64)
    })?
}

#[napi_derive::napi]
fn schema_cache_get(model: String, tool_set_hash: i64) -> napi::Result<Option<String>> {
    wrap_napi(|| {
        let cache = schema_cache();
        Ok(cache.get(&model, tool_set_hash as u64))
    })?
}

#[napi_derive::napi]
fn schema_cache_put(
    model: String,
    tool_set_hash: i64,
    schema_json: String,
    ttl_secs: i64,
) -> napi::Result<()> {
    wrap_napi(|| {
        let cache = schema_cache();
        cache.put(&model, tool_set_hash as u64, schema_json, ttl_secs as u64);
        Ok(())
    })?
}

#[napi_derive::napi]
fn schema_cache_evict_expired() -> napi::Result<i64> {
    wrap_napi(|| {
        let _cache = schema_cache();
        Ok(_cache.evict_expired() as i64)
    })?
}

// ---------------------------------------------------------------------------
// State pool
// ---------------------------------------------------------------------------

#[napi_derive::napi]
fn state_pool_init(capacity: i64, default_ttl_secs: i64) -> napi::Result<i64> {
    wrap_napi(|| {
        let pool = StatePool::with_capacity(capacity, default_ttl_secs);
        let id = state_pool_registry().counter.fetch_add(1, Ordering::Relaxed);
        state_pool_registry()
            .pools
            .lock()
            .unwrap()
            .insert(id, pool);
        Ok(id)
    })?
}

#[napi_derive::napi]
fn state_pool_acquire(
    pool_id: i64,
    instance_id: String,
    config_json: String,
    ttl_secs: i64,
) -> napi::Result<String> {
    wrap_napi(|| {
        let registry = state_pool_registry();
        let pools = registry.pools.lock().unwrap();
        let pool = pools
            .get(&pool_id)
            .ok_or_else(|| napi::Error::from_reason("invalid state pool"))?;
        Ok(pool.acquire(&instance_id, &config_json, ttl_secs))
    })?
}

#[napi_derive::napi]
fn state_pool_release(pool_id: i64, instance_id: String) -> napi::Result<()> {
    wrap_napi(|| {
        let registry = state_pool_registry();
        let pools = registry.pools.lock().unwrap();
        let pool = pools
            .get(&pool_id)
            .ok_or_else(|| napi::Error::from_reason("invalid state pool"))?;
        pool.release(&instance_id);
        Ok(())
    })?
}

#[napi_derive::napi]
fn state_pool_prune(pool_id: i64) -> napi::Result<i64> {
    wrap_napi(|| {
        let registry = state_pool_registry();
        let pools = registry.pools.lock().unwrap();
        let pool = pools
            .get(&pool_id)
            .ok_or_else(|| napi::Error::from_reason("invalid state pool"))?;
        Ok(pool.prune())
    })?
}

#[napi_derive::napi]
fn state_pool_len(pool_id: i64) -> napi::Result<i64> {
    wrap_napi(|| {
        let registry = state_pool_registry();
        let pools = registry.pools.lock().unwrap();
        let pool = pools
            .get(&pool_id)
            .ok_or_else(|| napi::Error::from_reason("invalid state pool"))?;
        Ok(pool.len())
    })?
}

// ---------------------------------------------------------------------------
// Prompt arena
// ---------------------------------------------------------------------------

#[napi_derive::napi]
fn prompt_init() -> napi::Result<()> {
    wrap_napi(|| {
        let _ = prompt_arena();
        Ok(())
    })?
}

#[napi_derive::napi]
fn prompt_add_message(
    conversation_id: i64,
    role: String,
    content: String,
) -> napi::Result<i64> {
    wrap_napi(|| {
        let arena = prompt_arena();
        let role = parse_role(&role)?;
        let id = arena.add_message_to_conversation(conversation_id, role, content);
        Ok(id as i64)
    })?
}

#[napi_derive::napi]
fn prompt_materialize(conversation_id: i64) -> napi::Result<String> {
    wrap_napi(|| {
        let arena = prompt_arena();
        Ok(arena.materialize(conversation_id))
    })?
}

#[napi_derive::napi]
fn prompt_add_attachment(
    _conversation_id: i64,
    attachment_id: String,
    bytes: Vec<u8>,
) -> napi::Result<()> {
    wrap_napi(|| {
        let arena = prompt_arena();
        arena.add_attachment(attachment_id, bytes);
        Ok(())
    })?
}

#[napi_derive::napi]
fn prompt_get_attachment(attachment_id: String) -> napi::Result<Vec<u8>> {
    wrap_napi(|| {
        let arena = prompt_arena();
        match arena.get_attachment(&attachment_id) {
            Some(bytes) => Ok(bytes),
            None => Err(napi::Error::from_reason("attachment not found")),
        }
    })?
}

// ---------------------------------------------------------------------------
// Compaction
// ---------------------------------------------------------------------------

#[napi_derive::napi]
fn compact_spill(messages_json: String, target_bytes: i64) -> napi::Result<String> {
    wrap_napi(|| {
        let messages: Vec<NanoMessage> = serde_json::from_str(&messages_json).map_err(|e| {
            napi::Error::from_reason(e.to_string())
        })?;
        Ok(compact_messages(&messages, target_bytes))
    })?
}

#[napi_derive::napi]
fn compact_prune_old(messages_json: String, target_bytes: i64) -> napi::Result<String> {
    wrap_napi(|| {
        let messages: Vec<NanoMessage> = serde_json::from_str(&messages_json).map_err(|e| {
            napi::Error::from_reason(e.to_string())
        })?;
        Ok(compact_prune_messages(&messages, target_bytes))
    })?
}

// ---------------------------------------------------------------------------
// Instructions
// ---------------------------------------------------------------------------

#[napi_derive::napi]
fn instructions_load(path: String) -> napi::Result<String> {
    wrap_napi(|| {
        let store = instructions_store();
        let content = store.load(&path).map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(content.to_string())
    })?
}

#[napi_derive::napi]
fn instructions_evict(path: String) -> napi::Result<bool> {
    wrap_napi(|| {
        let store = instructions_store();
        Ok(store.evict(&path))
    })?
}
