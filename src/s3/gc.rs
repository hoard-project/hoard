#![allow(dead_code)]
//! Garbage collection: periodic cleanup of expired S3 backup objects via S3 client.
//!
//! Uses `mc` CLI (pre-installed) for listing and deletion. Avoids SigV4 signing
//! bugs and HTTP client quirks by delegating to a battle-tested tool.
//!
//! # Orphan cleanup (OnDelete)
//!
//! When `on_delete = "delete"` is set on a volume, GC also scans for S3 objects
//! whose local counterparts have been deleted. These orphans are removed from S3
//! regardless of TTL. `on_delete = "keep"` (default) leaves orphans untouched.

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use chrono::{Duration as ChronoDuration, Utc};
use std::path::Path;
use std::process::Command;
use std::time::Duration;

/// Statistics from a GC cycle.
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    pub deleted: u64,
    pub errors: u64,
    pub bytes_freed: u64,
    /// Orphaned objects deleted due to OnDelete policy.
    pub orphans_deleted: u64,
}

/// An S3 object entry parsed from `mc ls --json`.
#[derive(Debug)]
struct McObject {
    key: String,
    last_modified: String,
    size: u64,
}

/// Run a single GC cycle using `mc` CLI.
///
/// `mc_alias`: the pre-configured mc alias (e.g. "guser")
/// `bucket`: S3 bucket name (e.g. "hoard-backups")
/// `prefix`: S3 key prefix to scan (e.g. "nomad")
/// `ttl`: objects older than this are deleted
pub async fn gc_cycle_mc(
    mc_alias: &str,
    bucket: &str,
    prefix: &str,
    ttl: Duration,
) -> Result<GcStats> {
    let cutoff = Utc::now() - ChronoDuration::from_std(ttl).unwrap_or(ChronoDuration::days(7));

    // List objects under prefix using mc ls --json
    let list_path = format!("{mc_alias}/{bucket}/{prefix}/");
    let objects = list_objects(mc_alias, &list_path)?;

    let mut stats = GcStats::default();

    for obj in &objects {
        // Parse last_modified and check if expired
        let expired = parse_mc_timestamp(&obj.last_modified)
            .map(|ts| ts < cutoff)
            .unwrap_or_else(|| {
                tracing::warn!(key = %obj.key, last_modified = %obj.last_modified, "unparseable timestamp, skipping");
                false
            });

        if expired {
            let rm_path = format!("{mc_alias}/{bucket}/{prefix}/{}", obj.key);
            match delete_object(mc_alias, &rm_path) {
                Ok(()) => {
                    stats.deleted += 1;
                    stats.bytes_freed += obj.size;
                    tracing::info!(key = %obj.key, size = obj.size, "GC: deleted expired object");
                }
                Err(e) => {
                    stats.errors += 1;
                    tracing::error!(key = %obj.key, error = %e, "GC: delete failed");
                }
            }
        }
    }

    tracing::info!(
        deleted = stats.deleted,
        errors = stats.errors,
        bytes_freed = stats.bytes_freed,
        total = objects.len(),
        prefix,
        "GC cycle complete"
    );

    Ok(stats)
}

/// Clean up orphaned S3 objects — files deleted from disk but still in S3.
///
/// For each S3 object under `prefix`, checks if the corresponding local
/// file exists under `watch_root`. If not, deletes it from S3.
///
/// This is the OnDelete::Delete policy implementation.
pub fn gc_orphan_cleanup(
    mc_alias: &str,
    bucket: &str,
    prefix: &str,
    watch_root: &Path,
) -> Result<u64> {
    let list_path = format!("{mc_alias}/{bucket}/{prefix}/");
    let objects = list_objects(mc_alias, &list_path)?;

    let mut orphans = 0u64;

    for obj in &objects {
        // The S3 key is relative to prefix — reconstruct local path
        let local_path = watch_root.join(&obj.key);
        if !local_path.exists() {
            let rm_path = format!("{mc_alias}/{bucket}/{prefix}/{}", obj.key);
            match delete_object(mc_alias, &rm_path) {
                Ok(()) => {
                    orphans += 1;
                    tracing::info!(
                        key = %obj.key,
                        size = obj.size,
                        "GC: deleted orphaned S3 object (OnDelete::Delete)"
                    );
                }
                Err(e) => {
                    tracing::error!(key = %obj.key, error = %e, "GC: orphan delete failed");
                }
            }
        }
    }

    if orphans > 0 {
        tracing::info!(orphans, prefix, "GC: orphan cleanup complete");
    }

    Ok(orphans)
}

/// List S3 objects using `mc ls --json`.
fn list_objects(_mc_alias: &str, list_path: &str) -> Result<Vec<McObject>> {
    let output = Command::new("mc")
        .args(["ls", "--json", list_path])
        .output()
        .context("mc ls failed")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("mc ls returned non-zero: {stderr}");
    }

    parse_mc_json(&output.stdout)
}

/// Delete a single S3 object using `mc rm`.
fn delete_object(_mc_alias: &str, rm_path: &str) -> Result<()> {
    let out = Command::new("mc")
        .args(["rm", rm_path])
        .output()
        .context("mc rm failed")?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        anyhow::bail!("mc rm: {stderr}")
    }
}

/// Parse `mc ls --json` output (one JSON object per line).
fn parse_mc_json(stdout: &[u8]) -> Result<Vec<McObject>> {
    let mut objects = Vec::new();

    for line in stdout.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        // mc ls --json returns lines like:
        // {"key":"default/gc-expired.txt","lastModified":"2026-06-17T07:34:32.200Z","size":8,...}
        let v: serde_json::Value = serde_json::from_slice(line).with_context(|| {
            format!("mc ls JSON parse error: {}", String::from_utf8_lossy(line))
        })?;

        let key = v["key"].as_str().unwrap_or("").to_string();
        let last_modified = v["lastModified"].as_str().unwrap_or("").to_string();
        let size = v["size"].as_u64().unwrap_or(0);

        if !key.is_empty() {
            objects.push(McObject {
                key,
                last_modified,
                size,
            });
        }
    }

    Ok(objects)
}

/// Parse S3-compatible timestamp format: "2026-06-17T07:34:32.200Z"
fn parse_mc_timestamp(s: &str) -> Option<chrono::DateTime<Utc>> {
    // mc uses RFC3339 with milliseconds: "2026-06-17T07:34:32.200Z"
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

// ── Keep the old gc_cycle signature for compatibility ──

use crate::s3::VerifiedS3Backend;

/// Legacy wrapper — delegates to `gc_cycle_mc` using the backend's config.
/// Uses `guser` alias (pre-configured S3 user for hoard).
pub async fn gc_cycle(s3: &VerifiedS3Backend, prefix: &str, ttl: Duration) -> Result<GcStats> {
    gc_cycle_mc("guser", s3.bucket_name(), prefix, ttl).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_mc_json_single() {
        let input =
            br#"{"key":"nomad/test.db","lastModified":"2026-06-17T07:34:32.200Z","size":8192}"#;
        let objs = parse_mc_json(input).unwrap();
        assert_eq!(objs.len(), 1);
        assert_eq!(objs[0].key, "nomad/test.db");
        assert_eq!(objs[0].size, 8192);
    }

    #[test]
    fn parse_mc_json_multiple() {
        let input = br#"{"key":"a.txt","lastModified":"2026-01-01T00:00:00Z","size":1}
{"key":"b.txt","lastModified":"2026-06-17T12:00:00Z","size":2}"#;
        let objs = parse_mc_json(input).unwrap();
        assert_eq!(objs.len(), 2);
    }

    #[test]
    fn parse_mc_timestamp_valid() {
        let ts = parse_mc_timestamp("2026-06-17T07:34:32.200Z");
        assert!(ts.is_some());
    }

    #[test]
    fn parse_mc_timestamp_invalid() {
        assert!(parse_mc_timestamp("not-a-date").is_none());
    }
}
