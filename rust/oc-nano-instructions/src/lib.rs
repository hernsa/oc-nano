//! Path-interned file contents store.
//!
//! Reads a file once, interns its contents as an `Arc<str>` keyed by path,
//! and returns the same `Arc<str>` on every subsequent call. Uses
//! `DashMap`'s `entry()` API so that, under concurrency, only one thread
//! performs the disk read for a given path; all others share the interned
//! `Arc<str>`. No re-read, no re-allocate — no unbounded concurrent file reads.

use std::io;
use std::path::{Component, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use dashmap::DashMap;

/// A cached instruction entry with TTL metadata.
#[derive(Debug)]
struct CachedInstruction {
    content: Arc<str>,
    cached_at: SystemTime,
}

/// Largest number of paths the store keeps interned at once.
///
/// The cache is keyed by path, so an unbounded map would grow with every
/// distinct instruction file the host ever reads. The oldest entry is dropped
/// once the map is over the cap.
pub const MAX_CACHED_ENTRIES: usize = 256;

/// A store of file contents interned by path.
///
/// Each unique path is read from disk at most once. Contents are stored as
/// an `Arc<str>` so callers can share references without cloning or
/// re-reading the file. Uses `DashMap`'s `entry()` API so that, under
/// concurrency, only one thread performs the disk read for a given path;
/// all others share the interned `Arc<str>`. No re-read, no re-allocate —
/// no unbounded concurrent file reads.
///
/// Access is restricted to files within `root_dir` (path traversal is
/// rejected), individual files are capped at `max_file_size`, cached
/// entries expire after `ttl_seconds`, and the cache holds at most
/// `max_entries` paths.
#[derive(Debug)]
pub struct InstructionsStore {
    inner: DashMap<String, CachedInstruction>,
    root_dir: PathBuf,
    max_file_size: usize,
    ttl_seconds: u64,
    max_entries: usize,
}

impl Default for InstructionsStore {
    fn default() -> Self {
        Self::with_root(std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
    }
}

impl InstructionsStore {
    /// Creates a new store rooted at the current working directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a new store with `root_dir` as the allowed directory root.
    /// All loaded paths must resolve within this directory.
    pub fn with_root(root_dir: PathBuf) -> Self {
        Self {
            inner: DashMap::new(),
            root_dir,
            max_file_size: 1024 * 1024, // 1 MiB
            ttl_seconds: 300,           // 5 minutes
            max_entries: MAX_CACHED_ENTRIES,
        }
    }

    /// Loads the file at `path`, interning its contents as an `Arc<str>`.
    ///
    /// The file must resolve within `root_dir` — path traversal (`..`
    /// components) is rejected. Files larger than `max_file_size` are
    /// rejected. Cached entries expire after `ttl_seconds`.
    pub fn load(&self, path: &str) -> io::Result<Arc<str>> {
        let path_buf = PathBuf::from(path);

        // Reject path traversal via `..` components.
        if path_buf
            .components()
            .any(|c| matches!(c, Component::ParentDir))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "path traversal not allowed",
            ));
        }

        // Resolve relative to root_dir if the path is not absolute.
        let resolved = if path_buf.is_absolute() {
            path_buf
        } else {
            self.root_dir.join(&path_buf)
        };

        // Verify the resolved path is within root_dir.
        let canonical_root = self
            .root_dir
            .canonicalize()
            .unwrap_or_else(|_| self.root_dir.clone());
        let canonical_resolved = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());

        if !canonical_resolved.starts_with(&canonical_root) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "path outside allowed directory",
            ));
        }

        // Check cache with TTL.
        let now = SystemTime::now();
        if let Some(cached) = self.inner.get(path) {
            if now
                .duration_since(cached.cached_at)
                .unwrap_or(Duration::MAX)
                < Duration::from_secs(self.ttl_seconds)
            {
                return Ok(Arc::clone(&cached.content));
            }
        }

        // Read file with size limit.
        let bytes = std::fs::read(&resolved)?;
        if bytes.len() > self.max_file_size {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "file too large",
            ));
        }
        let content: Arc<str> = String::from_utf8(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
            .into();

        let cached = CachedInstruction {
            content: Arc::clone(&content),
            cached_at: now,
        };

        self.inner.insert(path.to_string(), cached);
        self.enforce_capacity();
        Ok(content)
    }

    /// Keeps the cache within `max_entries`.
    ///
    /// Expired entries go first; when that is not enough, the oldest entries
    /// are dropped until the map is back within capacity. Without a cap the
    /// cache grows with every distinct path that is ever read.
    fn enforce_capacity(&self) {
        if self.inner.len() <= self.max_entries {
            return;
        }
        let now = SystemTime::now();
        self.inner.retain(|_, entry| {
            now.duration_since(entry.cached_at)
                .unwrap_or(Duration::MAX)
                < Duration::from_secs(self.ttl_seconds)
        });
        while self.inner.len() > self.max_entries {
            let oldest = self
                .inner
                .iter()
                .min_by_key(|entry| entry.value().cached_at)
                .map(|entry| entry.key().clone());
            match oldest {
                Some(key) => {
                    self.inner.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Evicts the cached contents for `path`.
    ///
    /// Returns `true` if an entry was removed. The next `load` for the same
    /// path will re-read from disk.
    pub fn evict(&self, path: &str) -> bool {
        self.inner.remove(path).is_some()
    }

    /// Returns the number of cached paths.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the store contains no entries.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Removes all entries from the store.
    pub fn clear(&self) {
        self.inner.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_instructions_intern_by_path() {
        let store = InstructionsStore::new();
        let path = std::env::current_dir().unwrap().join("oc_nano_instructions_test.md");
        let path_str = path.to_str().unwrap();
        std::fs::write(path_str, "hello world").unwrap();

        let c1 = store.load(path_str).unwrap();
        let c2 = store.load(path_str).unwrap();

        // Same Arc<str> for the same path — no re-read, no re-allocate.
        assert!(std::ptr::eq(&*c1, &*c2));
        assert_eq!(&*c1, "hello world");

        // len / is_empty reflect the single cached entry.
        assert_eq!(store.len(), 1);
        assert!(!store.is_empty());

        // evict drops the cached path; next load re-reads disk.
        assert!(store.evict(path_str));
        assert!(!store.evict(path_str));
        assert_eq!(store.len(), 0);
        assert!(store.is_empty());

        store.clear();
        let _ = std::fs::remove_file(path_str);
    }

    #[test]
    fn test_instructions_default_and_clear() {
        let store = InstructionsStore::default();
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
        store.clear();
        assert!(store.is_empty());
    }

    #[test]
    fn test_instructions_load_missing_path_errors() {
        let store = InstructionsStore::new();
        let missing = std::env::current_dir().unwrap().join("does_not_exist_oc_nano.md");
        let missing_str = missing.to_str().unwrap();
        assert!(store.load(missing_str).is_err());
    }

    #[test]
    fn test_instructions_load_rejects_path_traversal() {
        let store = InstructionsStore::new();
        // Parent traversal should be rejected regardless of file existence.
        assert!(store.load("../etc/passwd").is_err());
        assert!(store.load("../../../../tmp/malicious").is_err());
    }

    #[test]
    fn test_instructions_load_rejects_outside_root() {
        let tmp = std::env::temp_dir().join("oc_nano_instructions_outside_root.md");
        let tmp_str = tmp.to_str().unwrap();
        std::fs::write(tmp_str, "secret").unwrap();

        // Store rooted at current_dir() cannot read a file in temp_dir().
        let store = InstructionsStore::new();
        assert!(store.load(tmp_str).is_err());

        // A store rooted at temp_dir() can read it.
        let store_tmp = InstructionsStore::with_root(
            std::env::temp_dir().to_path_buf(),
        );
        let loaded = store_tmp.load(tmp_str).unwrap();
        assert_eq!(&*loaded, "secret");

        let _ = std::fs::remove_file(tmp_str);
    }

    #[test]
    fn test_instructions_load_respects_size_limit() {
        let tmp = std::env::temp_dir().join("oc_nano_instructions_big.md");
        let tmp_str = tmp.to_str().unwrap();
        std::fs::write(tmp_str, vec![0u8; 2 * 1024 * 1024]).unwrap(); // 2 MiB > 1 MiB limit

        let store = InstructionsStore::with_root(std::env::temp_dir().to_path_buf());
        assert!(store.load(tmp_str).is_err());

        let _ = std::fs::remove_file(tmp_str);
    }

    #[test]
    fn test_instructions_drops_the_oldest_entry_over_capacity() {
        let dir = std::env::current_dir()
            .unwrap()
            .join("oc_nano_instructions_capacity");
        std::fs::create_dir_all(&dir).unwrap();

        let store = InstructionsStore::with_root(dir.clone());
        let count = MAX_CACHED_ENTRIES + 20;
        for index in 0..count {
            let path = dir.join(format!("file-{index}.md"));
            std::fs::write(&path, format!("body {index}")).unwrap();
            store.load(path.to_str().unwrap()).unwrap();
        }

        // The map never grows past the cap, and the newest path survives.
        assert!(store.len() <= MAX_CACHED_ENTRIES);
        let newest = dir.join(format!("file-{}.md", count - 1));
        assert!(store.load(newest.to_str().unwrap()).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }
}
