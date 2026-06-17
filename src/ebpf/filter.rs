//! Path-based file filter for Hoard monitoring.
//!
//! Operates in userspace after inode→path resolution, before debounce.
//! Two-layer filtering:
//!   1. Directory scope: path must be under `watch_root`
//!   2. Glob patterns: file name must match at least one include pattern
//!   3. Exclude patterns: file name must NOT match any exclude pattern
//!
//! Configuration flows from CLI/env → `Config.watch_patterns` + `watch_excludes`.

#![deny(unsafe_code)]

use std::path::Path;

/// Compiled file filter for deciding which files to monitor.
#[derive(Debug, Clone)]
pub struct FileFilter {
    /// Root directory — only files under this path are monitored.
    watch_root: std::path::PathBuf,
    /// Compiled include glob patterns (from `watch_patterns`).
    include_globs: Vec<glob::Pattern>,
    /// Compiled exclude glob patterns (from `watch_excludes`, if any).
    exclude_globs: Vec<glob::Pattern>,
}

impl FileFilter {
    /// Build a filter from configuration.
    ///
    /// `watch_root` is canonicalized. `patterns` is a list of glob patterns
    /// (e.g. `["*.db", "*.db-wal"]`). `excludes` is a list of exclusion patterns.
    /// Empty slices match everything with no exclusions.
    pub fn new(
        watch_root: std::path::PathBuf,
        patterns: &[String],
        excludes: &[String],
    ) -> Result<Self, glob::PatternError> {
        let include_globs = if patterns.is_empty() {
            vec![glob::Pattern::new("*")?]
        } else {
            patterns
                .iter()
                .map(|s| glob::Pattern::new(s))
                .collect::<Result<Vec<_>, _>>()?
        };

        let exclude_globs = excludes
            .iter()
            .map(|s| glob::Pattern::new(s))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self {
            watch_root,
            include_globs,
            exclude_globs,
        })
    }

    /// Check whether `path` should be monitored.
    ///
    /// Returns `true` if the file:
    ///   1. Lives under `watch_root`
    ///   2. Its file name matches at least one include glob
    ///   3. Its file name matches zero exclude globs
    ///
    /// The path should already be canonicalized by the caller (debounce does this).
    pub fn should_monitor(&self, path: &Path) -> bool {
        // Layer 1: directory scope
        if !path.starts_with(&self.watch_root) {
            return false;
        }

        // Layer 2: file name check
        let file_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name,
            None => return false,
        };

        // Must match at least one include pattern
        if !self.include_globs.iter().any(|pat| pat.matches(file_name)) {
            return false;
        }

        // Must NOT match any exclude pattern
        if self.exclude_globs.iter().any(|pat| pat.matches(file_name)) {
            return false;
        }

        true
    }

    /// Return the watch root for diagnostic purposes.
    pub fn watch_root(&self) -> &Path {
        &self.watch_root
    }

    /// Number of include patterns (for metrics).
    pub fn include_count(&self) -> usize {
        self.include_globs.len()
    }

    /// Number of exclude patterns (for metrics).
    pub fn exclude_count(&self) -> usize {
        self.exclude_globs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn mk_filter(root: &str, inc: &[&str], exc: &[&str]) -> FileFilter {
        let inc: Vec<String> = inc.iter().map(|s| s.to_string()).collect();
        let exc: Vec<String> = exc.iter().map(|s| s.to_string()).collect();
        FileFilter::new(PathBuf::from(root), &inc, &exc).unwrap()
    }

    #[test]
    fn directory_scope_works() {
        let f = mk_filter("/data/db", &["*.db"], &[]);
        assert!(f.should_monitor(Path::new("/data/db/users.db")));
        assert!(!f.should_monitor(Path::new("/other/app.db")));
        assert!(!f.should_monitor(Path::new("/data/not-db/log.txt")));
    }

    #[test]
    fn glob_matching_works() {
        let f = mk_filter("/var/lib", &["*.db", "*.db-wal", "*.db-shm"], &[]);
        assert!(f.should_monitor(Path::new("/var/lib/app.db")));
        assert!(f.should_monitor(Path::new("/var/lib/sub/app.db-wal")));
        assert!(f.should_monitor(Path::new("/var/lib/app.db-shm")));
        assert!(!f.should_monitor(Path::new("/var/lib/config.toml")));
        assert!(!f.should_monitor(Path::new("/var/lib/notes.txt")));
    }

    #[test]
    fn exclude_overrides_include() {
        let f = mk_filter("/data", &["*.db"], &["*.tmp", "*_backup.db"]);
        assert!(f.should_monitor(Path::new("/data/live.db")));
        assert!(!f.should_monitor(Path::new("/data/cache.tmp")));
        assert!(!f.should_monitor(Path::new("/data/users_backup.db")));
    }

    #[test]
    fn empty_patterns_match_all() {
        let f = mk_filter("/app", &[], &[]);
        assert!(f.should_monitor(Path::new("/app/any.txt")));
        assert!(f.should_monitor(Path::new("/app/sub/dir/file.bin")));
        assert!(!f.should_monitor(Path::new("/outside/oops.txt")));
    }

    #[test]
    fn nested_directories_work() {
        let f = mk_filter("/mnt/volumes", &["*.sqlite", "*.db"], &[]);
        assert!(f.should_monitor(Path::new("/mnt/volumes/service-a/data.sqlite")));
        assert!(f.should_monitor(Path::new("/mnt/volumes/service-b/sub/db/app.db")));
        assert!(!f.should_monitor(Path::new("/mnt/volumes/service-a/config.json")));
    }

    #[test]
    fn no_filename_returns_false() {
        let f = mk_filter("/data", &["*.db"], &[]);
        assert!(!f.should_monitor(Path::new("/data/")));
        assert!(!f.should_monitor(Path::new("/data")));
    }

    #[test]
    fn invalid_pattern_returns_error() {
        let r = FileFilter::new(PathBuf::from("/"), &["[invalid".to_string()], &[]);
        assert!(r.is_err());
    }
}
