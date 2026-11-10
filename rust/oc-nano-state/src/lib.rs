//! oc-nano-state — bounded LRU+TTL instance pool for MCP/LSP/plugin instances.
//!
//! A fixed-capacity pool of named instances (MCP servers, LSP plugins, etc.)
//! with time-to-live expiration. The pool is strictly bounded: acquiring a new
//! instance evicts the least-recently-used entry when over capacity, and
//! `prune` drops TTL-expired entries plus any LRU entries still over capacity.
//!
//! Every method guards its work with `std::sync::Mutex`, so the pool is safe
//! to expose through napi-rs FFI from multiple threads. There are no
//! unbounded, process-lifetime maps here — growth is capped by `capacity`.

use std::num::NonZeroUsize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lru::LruCache;

fn non_zero(cap: usize) -> NonZeroUsize {
    // `with_capacity` clamps to >= 1, so this is always sound.
    NonZeroUsize::new(cap).expect("state pool capacity must be >= 1")
}

/// Default maximum number of pooled instances.
pub const DEFAULT_CAPACITY: usize = 64;
/// Default TTL in seconds for a pooled instance.
pub const DEFAULT_TTL_SECS: i64 = 300;

/// A single pooled instance.
#[derive(Clone, Debug)]
pub struct PooledInstance {
    pub instance_id: String,
    pub config_json: String,
    pub created_at: Instant,
    pub last_access: Instant,
    pub ttl: Duration,
}

impl PooledInstance {
    fn is_expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.created_at) >= self.ttl
    }
}

/// Bounded LRU+TTL instance pool.
pub struct StatePool {
    capacity: usize,
    default_ttl: Duration,
    cache: Mutex<LruCache<String, PooledInstance>>,
}

impl StatePool {
    /// Create a pool with `capacity` instances and a default TTL of
    /// `default_ttl_secs` seconds. Capacity is clamped to at least 1.
    pub fn with_capacity(capacity: i64, default_ttl_secs: i64) -> Self {
        let capacity = capacity.max(1) as usize;
        let default_ttl = Duration::from_secs(default_ttl_secs.max(0) as u64);
        Self {
            capacity,
            default_ttl,
            cache: Mutex::new(LruCache::new(non_zero(capacity))),
        }
    }

    /// Initialize a pool. Alias for [`with_capacity`] (mirrors the FFI
    /// `state_pool_init(capacity, default_ttl_secs)` binding).
    pub fn init(capacity: i64, default_ttl_secs: i64) -> Self {
        Self::with_capacity(capacity, default_ttl_secs)
    }

    /// Create or refresh an instance and return its config JSON.
    ///
    /// When `instance_id` already exists it is refreshed (config, TTL and
    /// access time updated) and moved to the most-recently-used position.
    /// When it is new it is inserted; if that pushes the pool over capacity
    /// the least-recently-used instance is evicted automatically.
    pub fn acquire(&self, instance_id: &str, config_json: &str, ttl_secs: i64) -> String {
        let ttl = Duration::from_secs(ttl_secs.max(0) as u64);
        let now = Instant::now();

        let fresh = PooledInstance {
            instance_id: instance_id.to_string(),
            config_json: config_json.to_string(),
            created_at: now,
            last_access: now,
            ttl,
        };

        let mut cache = self.cache.lock().unwrap();

        // `put` updates an existing key in place (no eviction) and inserts a
        // new key, auto-evicting the LRU entry when over capacity.
        cache.put(instance_id.to_string(), fresh);

        config_json.to_string()
    }

    /// Release (remove) an instance from the pool.
    pub fn release(&self, instance_id: &str) {
        let mut cache = self.cache.lock().unwrap();
        let mut next = LruCache::new(non_zero(self.capacity));
        for (key, inst) in cache.iter() {
            if key != instance_id {
                next.put(key.clone(), inst.clone());
            }
        }
        *cache = next;
    }

    /// Peek at an instance's config JSON without marking it recently used.
    /// Returns `None` when absent or TTL-expired.
    pub fn peek(&self, instance_id: &str) -> Option<String> {
        let mut cache = self.cache.lock().unwrap();
        let now = Instant::now();
        cache
            .get(instance_id)
            .filter(|inst| !inst.is_expired(now))
            .map(|inst| inst.config_json.clone())
    }

    /// Remove TTL-expired instances and evict LRU entries until within
    /// capacity. Returns the number of instances removed.
    pub fn prune(&self) -> i64 {
        let mut cache = self.cache.lock().unwrap();
        let now = Instant::now();
        let before = cache.len() as i64;

        let mut next = LruCache::new(non_zero(self.capacity));
        for (key, inst) in cache.iter() {
            if inst.is_expired(now) {
                continue; // drop expired
            }
            next.put(key.clone(), inst.clone()); // auto-evicts LRU if over capacity
        }
        *cache = next;

        before - cache.len() as i64
    }

    /// Current number of pooled instances.
    pub fn len(&self) -> i64 {
        self.cache.lock().unwrap().len() as i64
    }

    /// Maximum number of instances the pool can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Whether the pool currently holds no instances.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for StatePool {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY as i64, DEFAULT_TTL_SECS)
    }
}
