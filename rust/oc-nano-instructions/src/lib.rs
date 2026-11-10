//! Path-interned file contents store.
//!
//! Reads a file once, interns its contents as an `Arc<str>` keyed by path,
//! and returns the same `Arc<str>` on every subsequent call. Uses
//! `DashMap`'s `entry()` API so that, under concurrency, only one thread
//! performs the disk read for a given path; all others share the interned
//! `Arc<str>`. No re-read, no re-allocate — no unbounded concurrent file reads.

use std::io;
use std::sync::Arc;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;

/// A store of file contents interned by path.
///
/// Each unique path is read from disk at most once. Contents are stored as
/// an `Arc<str>` so callers can share references without cloning or
/// re-reading the file.
#[derive(Default, Debug)]
pub struct InstructionsStore {
    inner: DashMap<String, Arc<str>>,
}

impl InstructionsStore {
    /// Creates a new, empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads the file at `path`, interning its contents as an `Arc<str>`.
    ///
    /// The file is read from disk at most once per path. The `entry()` API
    /// guarantees a single read under concurrency: the first caller to reach
    /// a vacant entry performs the read and inserts the result; every other
    /// caller finds the entry occupied and receives a clone of the same
    /// `Arc<str>`. Subsequent calls never re-read or re-allocate.
    pub fn load(&self, path: &str) -> io::Result<Arc<str>> {
        match self.inner.entry(path.to_string()) {
            Entry::Occupied(occupied) => Ok(Arc::clone(occupied.get())),
            Entry::Vacant(vacant) => {
                let bytes = std::fs::read(vacant.key())?;
                let content: Arc<str> = String::from_utf8(bytes)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?
                    .into();
                let stored = vacant.insert(content);
                Ok(Arc::clone(&stored))
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
        let path = std::env::temp_dir().join("oc_nano_instructions_test.md");
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
        let missing = std::env::temp_dir().join("does_not_exist_oc_nano.md");
        let missing_str = missing.to_str().unwrap();
        assert!(store.load(missing_str).is_err());
    }
}
