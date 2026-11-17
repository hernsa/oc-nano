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
//!
//! The cache is bounded: `max_entries` and `max_bytes` cap total memory.
//! When a `put` would exceed either budget, the least-recently-inserted
//! entries are evicted first (by `cached_at`), then expired entries are swept.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use dashmap::DashMap;
use fxhash::FxHasher;
use std::hash::Hasher;

use oc_nano_core::SchemaCacheEntry;

/// Default caps: 256 entries / 16 MiB of serialized schema JSON.
const DEFAULT_MAX_ENTRIES: usize = 256;
const DEFAULT_MAX_BYTES: usize = 16 * 1024 * 1024;

/// A single cached schema with TTL metadata.
#[derive(Clone, Debug)]
pub struct CachedSchema {
    /// Serialized schema JSON — stored once, never deep-copied on read.
    pub schema_json: String,
    /// When the entry was inserted.
    pub cached_at: Instant,
    /// Time-to-live for the entry.
    pub ttl: Duration,
    /// Byte size of `schema_json` for budget accounting.
    pub byte_len: usize,
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
        let byte_len = entry.schema_json.len();
        Self {
            schema_json: entry.schema_json,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(entry.ttl_secs),
            byte_len,
        }
    }
}

/// In-memory tool-schema cache.
///
/// Keyed by `(model, tool_set_hash)`. Uses `DashMap` for concurrent access
/// without locking the whole table. Bounded by `max_entries` and `max_bytes`;
/// over-budget inserts evict the least-recently-inserted entries first.
#[derive(Debug)]
pub struct SchemaCache {
    cache: DashMap<(String, u64), CachedSchema>,
    bytes: AtomicUsize,
    max_entries: usize,
    max_bytes: usize,
}

impl Default for SchemaCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SchemaCache {
    /// Creates an empty `SchemaCache` with default capacity caps.
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_MAX_ENTRIES, DEFAULT_MAX_BYTES)
    }

    /// Creates an empty `SchemaCache` with explicit capacity caps.
    pub fn with_capacity(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            cache: DashMap::new(),
            bytes: AtomicUsize::new(0),
            max_entries,
            max_bytes,
        }
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
    /// entries are removed on lookup and their bytes are subtracted from the
    /// running budget.
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
            self.bytes.fetch_sub(value.len(), Ordering::Relaxed);
            None
        } else {
            Some(value)
        }
    }

    /// Insert a schema with a TTL (in seconds).
    ///
    /// If an entry already exists for the same key its bytes are subtracted
    /// first. After inserting, evicts to budget: drops the oldest entries
    /// (by `cached_at`) until both `max_entries` and `max_bytes` are satisfied.
    pub fn put(
        &self,
        model: &str,
        tool_set_hash: u64,
        schema_json: String,
        ttl_secs: u64,
    ) {
        let key = (model.to_string(), tool_set_hash);
        let byte_len = schema_json.len();
        let entry = CachedSchema {
            schema_json,
            cached_at: Instant::now(),
            ttl: Duration::from_secs(ttl_secs),
            byte_len,
        };

        // If replacing an existing entry, subtract its old bytes first.
        let old_bytes = self.cache.get(&key).map(|g| g.byte_len).unwrap_or(0);

        self.cache.insert(key.clone(), entry);
        self.bytes.fetch_add(byte_len.saturating_sub(old_bytes), Ordering::Relaxed);

        self.evict_to_budget();
    }

    /// Evict the oldest (by `cached_at`) entries until the cache is within both
    /// the entry count and byte budget.
    fn evict_to_budget(&self) {
        while self.bytes.load(Ordering::Relaxed) > self.max_bytes || self.cache.len() > self.max_entries {
            let oldest_key = self
                .cache
                .iter()
                .min_by_key(|entry| entry.value().cached_at)
                .map(|entry| entry.key().clone());
            let Some(key) = oldest_key else { break };
            if let Some((_, removed)) = self.cache.remove(&key) {
                self.bytes.fetch_sub(removed.byte_len, Ordering::Relaxed);
            }
        }
    }

    /// Remove all expired entries. Returns the number of entries removed.
    pub fn evict_expired(&self) -> usize {
        let mut removed = 0usize;
        self.cache.retain(|_, entry| {
            let keep = !entry.is_expired();
            if !keep {
                removed += 1;
                self.bytes.fetch_sub(entry.byte_len, Ordering::Relaxed);
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
        self.bytes.store(0, Ordering::Relaxed);
    }
}

/// Free-function wrapper so `hash_tool_set` can be called without an
/// instance, matching the crate's public interface.
pub fn hash_tool_set(tools_json: &str) -> u64 {
    SchemaCache::hash_tool_set(tools_json)
}
