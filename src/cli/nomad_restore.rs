//! `nomad-restore` — Nomad-aware bulk restore.
//!
//! Designed as a **prestart hook**: runs before the application task,
//! pulls the latest backup from S3, and ensures data is in place
//! before the app starts.
//!
//! Auto-detection (from environment):
//!   - S3 endpoint/bucket/keys from HOARD_S3_* or NOMAD_META_hoard_*
//!   - Restore prefix from NOMAD_META_hoard_prefix
//!   - Destination from NOMAD_ALLOC_DIR
//!
//! Usage (typically in Nomad job spec, never manual):
//!   hoard nomad-restore
//!   hoard nomad-restore --if-empty          # skip if dir non-empty (first deploy)
//!   hoard nomad-restore --prefix backup/x   # override auto-detected prefix

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::process::Command;

#[derive(Parser, Debug)]
pub struct NomadRestoreArgs {
    /// S3 endpoint URL
    #[arg(long, env = "HOARD_S3_ENDPOINT")]
    s3_endpoint: Option<String>,

    /// S3 region
    #[arg(long, env = "HOARD_S3_REGION", default_value = "us-east-1")]
    s3_region: String,

    /// S3 bucket
    #[arg(long, env = "HOARD_S3_BUCKET")]
    s3_bucket: Option<String>,

    /// S3 access key
    #[arg(long, env = "HOARD_S3_ACCESS_KEY", hide_env_values = true)]
    s3_access_key: Option<String>,

    /// S3 secret key
    #[arg(long, env = "HOARD_S3_SECRET_KEY", hide_env_values = true)]
    s3_secret_key: Option<String>,

    /// S3 prefix to restore from (default: read from NOMAD_META_hoard_prefix)
    #[arg(long)]
    prefix: Option<String>,

    /// Destination directory (default: NOMAD_ALLOC_DIR)
    #[arg(long)]
    dest: Option<PathBuf>,

    /// Skip restore if destination directory is non-empty (first deploy guard)
    #[arg(long)]
    if_empty: bool,

    /// Force overwrite existing files
    #[arg(long)]
    force: bool,

    /// Dry run — list what would be restored
    #[arg(long)]
    dry_run: bool,
}

/// Run the nomad-restore subcommand.
pub async fn run(args: NomadRestoreArgs) -> Result<()> {
    // ── Resolve S3 endpoint ──
    let endpoint = args.s3_endpoint.or_else(|| std::env::var("HOARD_S3_ENDPOINT").ok());
    let bucket = args
        .s3_bucket
        .or_else(|| std::env::var("HOARD_S3_BUCKET").ok())
        .unwrap_or_else(|| "guardian-backups".into());
    let access_key = args
        .s3_access_key
        .or_else(|| std::env::var("HOARD_S3_ACCESS_KEY").ok());
    let secret_key = args
        .s3_secret_key
        .or_else(|| std::env::var("HOARD_S3_SECRET_KEY").ok());

    // ── Resolve prefix ──
    let prefix = args.prefix.or_else(|| {
        std::env::var("NOMAD_META_hoard_prefix")
            .ok()
            .or_else(|| std::env::var("HOARD_S3_PREFIX").ok())
    });

    let prefix = match prefix {
        Some(p) => p,
        None => {
            // Fallback: derive from NOMAD_JOB_NAME
            let job = std::env::var("NOMAD_JOB_NAME").unwrap_or_else(|_| "unknown".into());
            format!("backup/{job}")
        }
    };

    // ── Resolve destination ──
    let dest = args.dest.or_else(|| {
        std::env::var("NOMAD_ALLOC_DIR")
            .ok()
            .map(PathBuf::from)
    });

    let dest = match dest {
        Some(d) => d,
        None => {
            anyhow::bail!(
                "No destination directory: set --dest or NOMAD_ALLOC_DIR environment variable"
            );
        }
    };

    // ── if-empty guard ──
    if args.if_empty && dest.exists() {
        let has_files = std::fs::read_dir(&dest)
            .map(|mut rd| rd.next().is_some())
            .unwrap_or(false);
        if has_files {
            tracing::info!(
                dest = %dest.display(),
                "destination non-empty, skipping restore (--if-empty)"
            );
            return Ok(());
        }
    }

    // ── Create destination ──
    std::fs::create_dir_all(&dest)
        .with_context(|| format!("failed to create {}", dest.display()))?;

    // ── Set up mc credentials if provided ──
    let mc_alias = "guser";
    if let (Some(ref ak), Some(ref sk), Some(ref ep)) = (&access_key, &secret_key, &endpoint) {
        // Register or update mc alias
        let result = Command::new("mc")
            .args([
                "alias",
                "set",
                mc_alias,
                ep,
                ak,
                sk,
            ])
            .output();

        match result {
            Ok(out) if out.status.success() => {
                tracing::debug!(alias = mc_alias, "mc alias configured");
            }
            Ok(out) => {
                let stderr = String::from_utf8_lossy(&out.stderr);
                tracing::warn!(%stderr, "mc alias set warning (may already exist)");
            }
            Err(e) => {
                tracing::warn!(%e, "mc alias set failed (using preconfigured alias)");
            }
        }
    }

    // ── List objects ──
    let list_path = format!("{mc_alias}/{bucket}/{prefix}/");
    tracing::info!(%list_path, "listing backup objects");

    let output = Command::new("mc")
        .args(["ls", "--recursive", "--json", &list_path])
        .output()
        .context("mc ls failed (is mc installed and configured?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("mc ls returned error: {stderr}");
    }

    let objects = parse_restore_objects(&output.stdout)?;
    tracing::info!(count = objects.len(), "found backup objects");

    if objects.is_empty() {
        tracing::info!(%prefix, "no backups found — first deploy?");
        return Ok(());
    }

    if args.dry_run {
        tracing::info!(count = objects.len(), "dry run — would restore");
        for obj in &objects {
            let dest_path = dest.join(&obj.local_path);
            let status = if dest_path.exists() { " (exists)" } else { "" };
            tracing::info!(key = %obj.key, dest = %dest_path.display(), "{status}");
        }
        return Ok(());
    }

    // ── Download each object ──
    let mut restored = 0u64;
    let mut skipped = 0u64;
    let mut errors = 0u64;

    for obj in &objects {
        let dest_path = dest.join(&obj.local_path);

        if dest_path.exists() && !args.force {
            tracing::debug!(path = %dest_path.display(), "skipping (exists)");
            skipped += 1;
            continue;
        }

        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let s3_path = format!("{mc_alias}/{bucket}/{prefix}/{}", obj.key);
        tracing::info!(key = %obj.key, dest = %dest_path.display(), "restoring");

        let result = Command::new("mc")
            .args(["cp", &s3_path, dest_path.to_string_lossy().as_ref()])
            .output();

        match result {
            Ok(out) if out.status.success() => {
                restored += 1;
            }
            Ok(out) => {
                errors += 1;
                let stderr = String::from_utf8_lossy(&out.stderr);
                tracing::error!(key = %obj.key, error = %stderr.trim(), "restore failed");
            }
            Err(e) => {
                errors += 1;
                tracing::error!(key = %obj.key, %e, "mc cp command failed");
            }
        }
    }

    if errors > 0 {
        anyhow::bail!(
            "restore incomplete: {} restored, {} skipped, {} errors ({} total)",
            restored,
            skipped,
            errors,
            objects.len()
        );
    }

    tracing::info!(
        restored,
        skipped,
        total = objects.len(),
        dest = %dest.display(),
        "restore complete"
    );

    Ok(())
}

/// Object entry from `mc ls --json`.
struct RestoreObject {
    key: String,
    local_path: PathBuf,
}

/// Parse `mc ls --json` output, extract keys, compute local paths relative to prefix.
fn parse_restore_objects(stdout: &[u8]) -> Result<Vec<RestoreObject>> {
    let mut objects = Vec::new();

    for line in stdout.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }

        let v: serde_json::Value = serde_json::from_slice(line).with_context(|| {
            format!("mc ls JSON parse error: {}", String::from_utf8_lossy(line))
        })?;

        let key = v["key"].as_str().unwrap_or("").to_string();
        if key.is_empty() || key.ends_with('/') {
            // Skip directory markers
            continue;
        }
        let local_path = PathBuf::from(&key);
        objects.push(RestoreObject { key, local_path });
    }

    Ok(objects)
}
