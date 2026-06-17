//! Stat-based dual-sample debounce for BPF events.
//!
//! When a BPF event fires, the file may still be actively written.
//! We stat the file, wait 100ms, stat again — if mtime+size are
//! stable, the file is truly quiet and ready for upload.

#![deny(unsafe_code)]

use anyhow::Result;
use std::path::Path;
use std::time::Duration;

/// Result of a debounce check.
#[derive(Debug, Clone)]
pub struct StableFile {
    /// Canonical path to the file
    pub path: std::path::PathBuf,
    /// File size in bytes
    pub size: u64,
    /// Last modification time (Unix epoch seconds)
    pub mtime_secs: i64,
}

/// Debouncer that performs dual-sample stat on file paths.
pub struct Debouncer {
    /// Wait duration between the two stat samples
    settle_duration: Duration,
}

impl Default for Debouncer {
    fn default() -> Self {
        Self {
            settle_duration: Duration::from_millis(100),
        }
    }
}

impl Debouncer {
    /// Create a debouncer with the default 100ms settle duration.
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a file is stable (not being written to).
    ///
    /// Returns `Some(StableFile)` if the file exists and is quiet.
    /// Returns `None` if the file was deleted between checks.
    /// Returns an error if the file cannot be stat'd.
    pub fn check_stable(&self, path: &Path) -> Result<Option<StableFile>> {
        // Canonicalize — if the file doesn't exist, it was deleted
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => return Ok(None),
        };

        // First stat
        let meta1 = match std::fs::metadata(&canonical) {
            Ok(m) => m,
            Err(_) => return Ok(None), // file deleted
        };

        let size1 = meta1.len();
        let mtime1 = file_mtime_secs(&meta1);

        // Wait for the settle duration
        std::thread::sleep(self.settle_duration);

        // Second stat
        let meta2 = match std::fs::metadata(&canonical) {
            Ok(m) => m,
            Err(_) => return Ok(None), // file deleted during wait
        };

        let size2 = meta2.len();
        let mtime2 = file_mtime_secs(&meta2);

        // Both size and mtime must be identical
        if size1 == size2 && mtime1 == mtime2 {
            Ok(Some(StableFile {
                path: canonical,
                size: size1,
                mtime_secs: mtime1,
            }))
        } else {
            tracing::debug!(
                path = %canonical.display(),
                size1, size2, mtime1, mtime2,
                "file still changing, debounce suppressed"
            );
            Ok(None)
        }
    }
}

/// Extract mtime as Unix epoch seconds from Metadata.
fn file_mtime_secs(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.mtime()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn stable_file_passes_debounce() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"stable content").unwrap();
        let path = tmp.path().to_path_buf();

        let debouncer = Debouncer::new();
        let result = debouncer.check_stable(&path).unwrap();

        assert!(result.is_some());
        let stable = result.unwrap();
        assert_eq!(stable.size, 14);
    }

    #[test]
    fn deleted_file_returns_none() {
        let tmp = NamedTempFile::new().unwrap();
        let path = tmp.path().to_path_buf();
        drop(tmp); // delete the file

        let debouncer = Debouncer::new();
        let result = debouncer.check_stable(&path).unwrap();
        assert!(result.is_none());
    }
}
