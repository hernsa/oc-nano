//! oc-nano-schema — tool schema cache with STABLE hash (FxHash) and TTL eviction.
//!
//! This cache replaces the JS-side `JSON.parse(JSON.stringify(inputSchema))`
//! deep-copy: a model's tool-set schema is serialized once and stored in a
//! `DashMap` keyed by `(model, tool_set_hash)`.
//!
//! The hash is computed with `fxhash::FxHasher` (fixed seed), so the same
//! tool-set JSON always yields the same `u64`, even in a different process.
//! This is the key fix for the `DefaultHasher` random-seed issue that caused
//! cache misses after every process restart.
//!
//! Entries carry a TTL (in seconds) and are evicted lazily on `get` and
//! eagerly via `evict_expired`. No input schema is ever deep-copied on read.

use std::time::{Duration, Instant};

use dashmap::DashMap;
use fxhash::FxHasher;
use std::hash::Hasher;

use oc_nano_core::SchemaCacheEntry;

/// A single cached schema with TTL metadata.
#[derive(Clone, Debug)]
pub struct CachedSchema {
    /// Serialized schema JSON — stored once, never deep-copied on read.
    pub schema_json: String,
    /// When the entry was inserted.
    pub cached_at: Instant,
    /// Time-to-live for the entry.
    pub ttl: Duration,
}

impl CachedSchema {
    /// Returns `true` if `cached_at + ttl` has passed.
    fn is_expired(&self) -> bool {
        self.cached_at.elapsed() > self.ttl
    }

    /// Build a `CachedSchema` from a core `SchemaCacheEntry`.
    ///
    /// The entry's `inserted_at` is not portable across runtimes, so
    /// `cached_at` is reset to `Instant::now()` and the TTL is taken from
    /// `entry.ttl_secs`.
    pub fn from_entry(entry: SchemaCacheEntry) -> Self {
        Self {
            schema_json: entry.schema_json,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(entry.ttl_secs),
        }
    }
}

/// In-memory tool-schema cache.
///
/// Keyed by `(model, tool_set_hash)`. Uses `DashMap` for concurrent access
/// without locking the whole table.
#[derive(Clone, Debug, Default)]
pub struct SchemaCache {
    cache: DashMap<(String, u64), CachedSchema>,
}

impl SchemaCache {
    /// Creates an empty `SchemaCache`.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute a cross-process-stable hash of a tool-set JSON string.
    ///
    /// Uses `fxhash::FxHasher` with its fixed seed, so the same input always
    /// yields the same `u64`, even in a different process. This is the key
    /// fix for the `DefaultHasher` random-seed cache-miss issue.
    pub fn hash_tool_set(tools_json: &str) -> u64 {
        let mut hasher = FxHasher::default();
        hasher.write(tools_json.as_bytes());
        hasher.finish()
    }

    /// Retrieve a cached schema by model and tool-set hash.
    ///
    /// Returns `None` if the entry is missing or its TTL has expired. Expired
    /// entries are removed on lookup.
    pub fn get(&self, model: &str, tool_set_hash: u64) -> Option<String> {
        let key = (model.to_string(), tool_set_hash);
        let (value, expired) = {
            let guard = match self.cache.get(&key) {
                Some(g) => g,
                None => return None,
            };
            (guard.schema_json.clone(), guard.is_expired())
        }; // guard dropped here, so `remove` below never holds a read lock
        if expired {
            let _ = self.cache.remove(&key);
            None
        } else {
            Some(value)
        }
    }

    /// Insert a schema with a TTL (in seconds).
    pub fn put(
        &self,
        model: &str,
        tool_set_hash: u64,
        schema_json: String,
        ttl_secs: u64,
    ) {
        let entry = CachedSchema {
            schema_json,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(ttl_secs),
        };
        self.cache.insert((model.to_string(), tool_set_hash), entry);
    }

    /// Remove all expired entries. Returns the number of entries removed.
    pub fn evict_expired(&self) -> usize {
        let mut removed = 0usize;
        self.cache.retain(|_, entry| {
            let keep = !entry.is_expired();
            if !keep {
                removed += 1;
            }
            keep
        });
        removed
    }

    /// Number of entries currently in the cache (including not-yet-evicted
    /// expired ones).
    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }

    pub fn clear(&self) {
        self.cache.clear();
    }
}

/// Free-function wrapper so `hash_tool_set` can be called without an
/// instance, matching the crate's public interface.
pub fn hash_tool_set(tools_json: &str) -> u64 {
    SchemaCache::hash_tool_set(tools_json)
}
