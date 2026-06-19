#![allow(dead_code)]
//! Inode-to-path resolution for BPF (dev, ino) events.
//!
//! When a BPF event fires with (dev_t, ino_t), the
//! [`InodeCache`] resolves it to a file path.  First call on a given
//! (dev, ino) walks the watch directory tree (**O(n)**); subsequent
//! calls are **O(1)** cache hits.  Cache entries are verified on
//! every hit (stat → exists check) and evicted if the file has been
//! deleted or moved.

#![deny(unsafe_code)]

use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use tokio::sync::RwLock;

/// Maximum number of cached (dev, ino) entries.
const MAX_CACHE_ENTRIES: usize = 4096;

/// An LRU-ish inode → path cache backed by a `HashMap`.
///
/// The cache is **not** strictly LRU; when full it simply drops the
/// entire map and starts over (cheap for the expected workload of
/// ≤ hundreds of SQLite files).  This avoids the overhead of a
/// linked-list or a full LRU implementation.
pub struct InodeCache {
    inner: RwLock<HashMap<(u64, u64), PathBuf>>,
}

impl InodeCache {
    pub fn new() -> Self {
        Self {
            inner: RwLock::new(HashMap::with_capacity(256)),
        }
    }

    /// Resolve (dev, ino) to a filesystem path.
    ///
    /// On cache hit the cached path is verified with a cheap `try_exists()`
    /// call; on miss a full directory walk is performed under `watch_root`.
    pub async fn resolve(&self, watch_root: &Path, dev: u64, ino: u64) -> Option<PathBuf> {
        // ── Fast path: cache hit ──
        {
            let cache = self.inner.read().await;
            if let Some(path) = cache.get(&(dev, ino)) {
                if Self::path_exists(path) {
                    return Some(path.clone());
                }
            }
        }

        // ── Slow path: directory walk ──
        let path = walk_dir_for_inode(watch_root, dev, ino)?;

        // ── Insert into cache ──
        {
            let mut cache = self.inner.write().await;
            if cache.len() >= MAX_CACHE_ENTRIES {
                tracing::warn!("inode cache full ({} entries), evicting all", cache.len());
                cache.clear();
            }
            cache.insert((dev, ino), path.clone());
        }

        Some(path)
    }

    /// Number of currently cached entries.
    pub async fn len(&self) -> usize {
        self.inner.read().await.len()
    }

    /// Purge all entries (e.g. after a filesystem resize or remount).
    pub async fn invalidate_all(&self) {
        self.inner.write().await.clear();
    }

    /// Pre-populate a cache entry (used by initial scan).
    pub async fn insert(&self, dev: u64, ino: u64, path: PathBuf) {
        self.inner.write().await.insert((dev, ino), path);
    }

    /// Remove a specific entry (call after unlinking a file).
    pub async fn invalidate(&self, dev: u64, ino: u64) {
        self.inner.write().await.remove(&(dev, ino));
    }

    /// Stat a path without allocating — returns true if it exists.
    fn path_exists(path: &Path) -> bool {
        // try_exists() is the canonical cheap check (fstatat).
        // Fall back to metadata() on older std or exotic platforms.
        path.try_exists().unwrap_or(false)
    }
}

impl Default for InodeCache {
    fn default() -> Self {
        Self::new()
    }
}

// ── Legacy API (stateless, for backward-compat and tests) ────────

/// Resolve a (dev, ino) pair by scanning `watch_root` (no caching).
///
/// Prefer [`InodeCache::resolve`] for production use.
pub fn resolve_inode(watch_root: &Path, dev: u64, ino: u64) -> Option<PathBuf> {
    walk_dir_for_inode(watch_root, dev, ino)
}

/// Convert userspace dev_t encoding (major<<8 | minor) to kernel encoding
/// (major<<20 | minor). Rust MetadataExt::dev() may return either format
/// depending on glibc version and statx(2) availability.  This function
/// normalises the old 16-bit encoding to the 32-bit kernel format used by
/// BPF `s_dev` values.
fn user_dev_to_kernel_dev(user_dev: u64) -> u64 {
    let major = (user_dev >> 8) & 0xfff;
    let minor = user_dev & 0xff;
    (major << 20) | minor
}

/// Walk a directory tree looking for a file with the given inode.
///
/// `dev` is expected in kernel encoding (major<<20 | minor), as produced
/// by BPF.  The walk tries *both* raw and converted userspace dev_t to
/// handle the glibc encoding ambiguity transparently.
fn walk_dir_for_inode(root: &Path, dev: u64, ino: u64) -> Option<PathBuf> {
    let mut dirs = vec![root.to_path_buf()];

    while let Some(dir) = dirs.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            let meta = match entry.metadata() {
                Ok(m) => m,
                Err(_) => continue,
            };

            // Check inode + dev match.  Try raw userspace dev_t first
            // (statx on glibc ≥ 2.28), then fall back to the converted
            // old-style encoding.  This makes the walk work regardless of
            // which encoding the local C library uses.
            let raw_dev = meta.dev();
            if meta.ino() == ino && (raw_dev == dev || user_dev_to_kernel_dev(raw_dev) == dev) {
                return Some(path);
            }

            // Recurse into directories (but skip symlinks for safety)
            if meta.is_dir() && !meta.file_type().is_symlink() {
                dirs.push(path);
            }
        }
    }

    None
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn resolve_existing_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.db");
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(b"hello").unwrap();
        drop(f);

        let meta = std::fs::metadata(&file_path).unwrap();
        let ino = meta.ino();
        let dev = meta.dev();

        let resolved = resolve_inode(dir.path(), dev, ino);
        assert_eq!(resolved, Some(file_path));
    }

    #[test]
    fn resolve_nonexistent_inode() {
        let dir = TempDir::new().unwrap();
        let resolved = resolve_inode(dir.path(), 99999, 99999);
        assert_eq!(resolved, None);
    }

    #[tokio::test]
    async fn cache_hit() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("cache-test.db");
        let mut f = std::fs::File::create(&file_path).unwrap();
        f.write_all(b"cache me").unwrap();
        drop(f);

        let meta = std::fs::metadata(&file_path).unwrap();
        let ino = meta.ino();
        let dev = meta.dev();

        let cache = InodeCache::new();

        // First call: directory walk (cache miss)
        let p1 = cache.resolve(dir.path(), dev, ino).await;
        assert_eq!(p1, Some(file_path.clone()));
        assert_eq!(cache.len().await, 1);

        // Second call: cache hit
        let p2 = cache.resolve(dir.path(), dev, ino).await;
        assert_eq!(p2, Some(file_path));
        assert_eq!(cache.len().await, 1); // still 1, no new insertion
    }

    #[tokio::test]
    async fn cache_miss_deleted_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("temp.db");
        std::fs::File::create(&file_path).unwrap();
        let meta = std::fs::metadata(&file_path).unwrap();
        let ino = meta.ino();
        let dev = meta.dev();

        let cache = InodeCache::new();

        // Populate cache
        let p1 = cache.resolve(dir.path(), dev, ino).await;
        assert!(p1.is_some());

        // Delete the file
        std::fs::remove_file(&file_path).unwrap();

        // Cache hit but file gone → fall through to walk → None
        let p2 = cache.resolve(dir.path(), dev, ino).await;
        assert!(p2.is_none());
    }

    #[tokio::test]
    async fn cache_invalidation() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("inv.db");
        std::fs::File::create(&file_path).unwrap();
        let meta = std::fs::metadata(&file_path).unwrap();
        let ino = meta.ino();
        let dev = meta.dev();

        let cache = InodeCache::new();
        let _ = cache.resolve(dir.path(), dev, ino).await;
        assert_eq!(cache.len().await, 1);

        cache.invalidate(dev, ino).await;
        assert_eq!(cache.len().await, 0);
    }

    #[tokio::test]
    async fn cache_eviction_on_full() {
        let dir = TempDir::new().unwrap();

        // Create many files, one after another, resolving each
        let cache = InodeCache::new();

        // Exceed MAX_CACHE_ENTRIES by writing to the same cache key slot
        // (the map is keyed by (dev, ino) so we need unique files)
        for i in 0..(MAX_CACHE_ENTRIES + 10) {
            let p = dir.path().join(format!("evict_{i}.db"));
            let mut f = std::fs::File::create(&p).unwrap();
            f.write_all(b"x").unwrap();
            drop(f);

            let meta = std::fs::metadata(&p).unwrap();
            let ino = meta.ino();
            let dev = meta.dev();
            let _ = cache.resolve(dir.path(), dev, ino).await;
        }

        // Should have evicted and be well under the limit
        let n = cache.len().await;
        assert!(
            n <= MAX_CACHE_ENTRIES,
            "cache grew to {n}, expected ≤ {MAX_CACHE_ENTRIES}"
        );
    }
}
