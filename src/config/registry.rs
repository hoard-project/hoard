//! Volume registry: match file paths to volumes and resolve config.
//!
//! The registry holds `ResolvedVolume` entries in priority order
//! (most-specific glob first).  When a BPF event triggers,
//! `registry.resolve(file_path)` returns the matching volume's
//! upload config.

use super::v2::ResolvedVolume;
use std::path::Path;
use std::sync::RwLock;

/// Lightweight volume registry for path-to-config resolution.
///
/// Internally uses a `RwLock<Vec<ResolvedVolume>>` so volumes can be
/// atomically reloaded (SIGHUP / Nomad meta refresh) without blocking
/// the BPF fast path.
#[derive(Debug)]
pub struct VolumeRegistry {
    volumes: RwLock<Vec<ResolvedVolume>>,
}

impl Clone for VolumeRegistry {
    fn clone(&self) -> Self {
        let guard = self.volumes.read().expect("VolumeRegistry read lock poisoned");
        Self {
            volumes: RwLock::new(guard.clone()),
        }
    }
}

impl VolumeRegistry {
    /// Create a new registry from resolved volumes.
    /// Volumes are sorted by glob specificity: longer patterns first.
    pub fn new(mut volumes: Vec<ResolvedVolume>) -> Self {
        volumes.sort_by(|a, b| {
            let specificity_a = glob_specificity(&a.match_glob);
            let specificity_b = glob_specificity(&b.match_glob);
            specificity_b.cmp(&specificity_a) // descending
        });
        Self { volumes: RwLock::new(volumes) }
    }

    /// Reload the volume list atomically (SIGHUP / Nomad meta refresh).
    pub fn reload(&self, mut new_volumes: Vec<ResolvedVolume>) {
        new_volumes.sort_by(|a, b| {
            let specificity_a = glob_specificity(&a.match_glob);
            let specificity_b = glob_specificity(&b.match_glob);
            specificity_b.cmp(&specificity_a)
        });
        let mut w = self.volumes.write().expect("VolumeRegistry write lock poisoned");
        *w = new_volumes;
    }

    /// Number of volumes in the registry.
    pub fn len(&self) -> usize {
        self.volumes.read().expect("VolumeRegistry read lock poisoned").len()
    }

    /// Iterate over all volumes in priority order (borrowed, locked for read).
    pub fn iter(&self) -> impl Iterator<Item = ResolvedVolume> {
        let guard = self.volumes.read().expect("VolumeRegistry read lock poisoned");
        let cloned = guard.clone();
        cloned.into_iter()
    }

    /// Iterate over all volumes in priority order (owned, for async).
    pub fn to_vec(&self) -> Vec<ResolvedVolume> {
        self.volumes.read().expect("VolumeRegistry read lock poisoned").clone()
    }

    /// Resolve a file path to its volume config.
    pub fn resolve(&self, file_path: &Path, watch_root: &Path) -> ResolvedVolume {
        let guard = self.volumes.read().expect("VolumeRegistry read lock poisoned");

        let rel = match file_path.strip_prefix(watch_root) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => file_path.to_string_lossy().to_string(),
        };
        let rel = rel.trim_start_matches("./");

        for vol in guard.iter() {
            if matches_glob(&vol.match_glob, rel) {
                return vol.clone();
            }
        }

        // Fallback: last volume (catch-all).
        guard.last().cloned().expect("VolumeRegistry must have at least one volume")
    }
}

/// Compute glob specificity — longer patterns are more specific.
fn glob_specificity(pattern: &str) -> usize {
    // Count non-wildcard characters as a rough specificity measure.
    pattern.chars().filter(|c| *c != '*' && *c != '?').count()
}

/// Parse TTL string like "30d", "7d", "365d", "90d" into a Duration.
pub fn parse_ttl(ttl: &str) -> std::time::Duration {
    let ttl = ttl.trim();
    if ttl.ends_with('d') {
        let days: u64 = ttl[..ttl.len()-1].parse().unwrap_or(30);
        std::time::Duration::from_secs(days * 86400)
    } else if ttl.ends_with('h') {
        let hours: u64 = ttl[..ttl.len()-1].parse().unwrap_or(24);
        std::time::Duration::from_secs(hours * 3600)
    } else {
        // Fallback: treat as raw seconds
        let secs: u64 = ttl.parse().unwrap_or(30 * 86400);
        std::time::Duration::from_secs(secs)
    }
}

/// Simple glob matching supporting ** and *.
///
/// - `**` matches any number of path segments (including zero).
/// - `*`  matches any characters within a single path segment.
/// - Literal characters match themselves.
fn matches_glob(pattern: &str, path: &str) -> bool {
    let pattern_segments: Vec<&str> = pattern.split('/').collect();
    let path_segments: Vec<&str> = path.split('/').collect();

    matches_glob_segments(&pattern_segments, &path_segments, 0, 0)
}

fn matches_glob_segments(
    pattern: &[&str],
    path: &[&str],
    pi: usize,
    si: usize,
) -> bool {
    if pi >= pattern.len() && si >= path.len() {
        return true;
    }
    if pi >= pattern.len() {
        return false;
    }

    let pseg = pattern[pi];

    if pseg == "**" {
        // ** matches zero or more segments.
        // Try matching zero segments (skip **).
        if matches_glob_segments(pattern, path, pi + 1, si) {
            return true;
        }
        // Try matching one or more segments.
        for next_si in si..path.len() {
            if matches_glob_segments(pattern, path, pi + 1, next_si + 1) {
                return true;
            }
        }
        return false;
    }

    if si >= path.len() {
        return false;
    }

    if pseg == "*" || pseg == path[si] {
        return matches_glob_segments(pattern, path, pi + 1, si + 1);
    }

    // Handle glob patterns like "*.log" within a single segment.
    if pseg.contains('*') {
        let mut pat_chars = pseg.chars();
        let mut path_chars = path[si].chars();
        loop {
            match (pat_chars.next(), path_chars.next()) {
                (Some('*'), Some(_)) => {
                    // Greedy match: consume until next literal matches.
                    let rest: String = pat_chars.clone().collect();
                    if rest.is_empty() {
                        return true;
                    }
                    let remaining_path: String = path_chars.clone().collect();
                    for j in 0..=remaining_path.len() {
                        if remaining_path[j..].starts_with(&rest) {
                            return matches_glob_segments(pattern, path, pi + 1, si + 1);
                        }
                    }
                    return false;
                }
                (Some(pc), Some(sc)) if pc == sc || pc == '?' => continue,
                (None, None) => {
                    return matches_glob_segments(pattern, path, pi + 1, si + 1);
                }
                _ => return false,
            }
        }
    }

    false
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_volume(name: &str, glob: &str) -> ResolvedVolume {
        ResolvedVolume {
            name: name.to_string(),
            match_glob: glob.to_string(),
            s3_prefix: name.to_string(),
            ttl: "30d".to_string(),
            retries: 5,
            extensions: vec!["*".to_string()],
            exclude: vec![],
            compression: None,
            encryption: false,
            on_stop: super::super::v2::OnStop::Drain,
            on_delete: super::super::v2::OnDelete::Keep,
        }
    }

    #[test]
    fn catch_all_matches_everything() {
        assert!(matches_glob("**", "foo/bar/baz.txt"));
        assert!(matches_glob("**", "foo"));
        assert!(matches_glob("**", ""));
    }

    #[test]
    fn specific_glob_first() {
        let volumes = vec![
            test_volume("specific", "postgres/**"),
            test_volume("catch-all", "**"),
        ];
        let registry = VolumeRegistry::new(volumes);

        let root = std::path::Path::new("/var/lib/hoard/volumes");
        let v = registry.resolve(
            std::path::Path::new("/var/lib/hoard/volumes/postgres/data.db"),
            root,
        );
        assert_eq!(v.name, "specific");
    }

    #[test]
    fn nested_globs_most_specific_wins() {
        let volumes = vec![
            test_volume("schema", "postgres/schema/**"),
            test_volume("postgres", "postgres/**"),
            test_volume("catch-all", "**"),
        ];
        let registry = VolumeRegistry::new(volumes);

        let root = std::path::Path::new("/var/lib/hoard/volumes");

        let v1 = registry.resolve(
            std::path::Path::new("/var/lib/hoard/volumes/postgres/schema/v2.sql"),
            root,
        );
        assert_eq!(v1.name, "schema");

        let v2 = registry.resolve(
            std::path::Path::new("/var/lib/hoard/volumes/postgres/data.db"),
            root,
        );
        assert_eq!(v2.name, "postgres");

        let v3 = registry.resolve(
            std::path::Path::new("/var/lib/hoard/volumes/app-logs/app.log"),
            root,
        );
        assert_eq!(v3.name, "catch-all");
    }

    #[test]
    fn extension_glob() {
        assert!(matches_glob("*.log", "app.log"));
        assert!(!matches_glob("*.log", "app.json"));
    }
}

mod tests_ttl {
    use super::parse_ttl;

    #[test]
    fn parse_days() {
        assert_eq!(parse_ttl("30d"), std::time::Duration::from_secs(30 * 86400));
        assert_eq!(parse_ttl("7d"), std::time::Duration::from_secs(7 * 86400));
        assert_eq!(parse_ttl("365d"), std::time::Duration::from_secs(365 * 86400));
    }

    #[test]
    fn parse_hours() {
        assert_eq!(parse_ttl("24h"), std::time::Duration::from_secs(24 * 3600));
    }

    #[test]
    fn parse_default() {
        let dur = parse_ttl("30d");
        assert_eq!(dur.as_secs(), 30 * 86400);
    }
}
