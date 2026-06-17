//! Restore command — bulk restore SQLite backups from S3 to local filesystem.
//!
//! Uses `mc` CLI for listing and downloading (avoids SigV4 complexity).
//! Respects the daemon's TOML config for S3 credentials and prefix.
//!
//! Usage:
//!   hoard restore --dest /var/lib/sqlite --prefix backup
//!   hoard restore --dest /tmp/restore --force  # overwrite existing

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;
use std::process::Command;

/// Bulk restore arguments.
#[derive(Parser, Debug)]
pub struct RestoreArgs {
    /// Path to TOML config file (optional — uses defaults if omitted)
    #[arg(long)]
    config: Option<PathBuf>,

    /// Destination directory
    #[arg(long, default_value = ".")]
    dest: PathBuf,

    /// S3 prefix to restore (overrides config file)
    #[arg(long)]
    prefix: Option<String>,

    /// Force overwrite existing files
    #[arg(long)]
    force: bool,

    /// Dry run — list what would be restored without downloading
    #[arg(long)]
    dry_run: bool,
}

/// Minimal TOML fragment for restore (only needs [s3] section).
#[derive(Deserialize)]
struct RestoreConfig {
    s3: S3Section,
}

#[derive(Deserialize)]
struct S3Section {
    #[serde(default)]
    endpoint: Option<String>,
    #[serde(default)]
    bucket: Option<String>,
    #[serde(default)]
    prefix: Option<String>,
}

/// Run the restore subcommand.
pub async fn run(args: RestoreArgs) -> Result<()> {
    // ── Load config ──────────────────────────────────────────────
    let (endpoint, bucket, prefix, mc_alias) = if let Some(ref cfg_path) = args.config {
        let content = std::fs::read_to_string(cfg_path).context("failed to read config file")?;
        let cfg: RestoreConfig = toml::from_str(&content).context("failed to parse TOML config")?;

        let bucket = cfg.s3.bucket.unwrap_or_else(|| "hoard-backups".into());
        let prefix = args
            .prefix
            .unwrap_or_else(|| cfg.s3.prefix.unwrap_or_else(|| "backup".into()));

        // Detect mc alias from endpoint
        let ep = cfg.s3.endpoint.clone();
        let alias =
            infer_alias(&ep.as_deref().unwrap_or("http://127.0.0.1:9000")).unwrap_or("guser");

        (cfg.s3.endpoint, bucket, prefix, alias.to_string())
    } else {
        let prefix = args.prefix.unwrap_or_else(|| "backup".into());
        (None, "hoard-backups".into(), prefix, "guser".into())
    };

    let _ = endpoint; // mc handles endpoint via alias

    // ── Create destination ───────────────────────────────────────
    std::fs::create_dir_all(&args.dest)
        .with_context(|| format!("failed to create {}", args.dest.display()))?;

    // ── List objects ─────────────────────────────────────────────
    let list_path = format!("{mc_alias}/{bucket}/{prefix}/");
    tracing::info!(list_path, "listing backup objects");

    let output = Command::new("mc")
        .args(["ls", "--json", &list_path])
        .output()
        .context("mc ls failed (is mc installed and configured?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("mc ls returned error: {stderr}");
    }

    let objects = parse_restore_objects(&output.stdout)?;
    tracing::info!(count = objects.len(), "found backup objects");

    if objects.is_empty() {
        println!("No backups found under prefix '{prefix}'");
        return Ok(());
    }

    if args.dry_run {
        println!("Would restore {} objects:", objects.len());
        for obj in &objects {
            let dest_path = args.dest.join(&obj.local_path);
            let status = if dest_path.exists() { " (exists)" } else { "" };
            println!("  {} → {}{status}", obj.key, dest_path.display());
        }
        return Ok(());
    }

    // ── Download each object ─────────────────────────────────────
    let mut restored = 0u64;
    let mut skipped = 0u64;
    let mut errors = 0u64;

    for obj in &objects {
        let dest_path = args.dest.join(&obj.local_path);

        // Skip existing unless --force
        if dest_path.exists() && !args.force {
            tracing::debug!(path = %dest_path.display(), "skipping (exists)");
            skipped += 1;
            continue;
        }

        // Create parent directories
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let s3_path = format!("{mc_alias}/{bucket}/{prefix}/{}", obj.key);
        tracing::info!(key = %obj.key, dest = %dest_path.display(), "restoring");

        let result = Command::new("mc")
            .args(["cp", &s3_path, &dest_path.to_string_lossy().to_string()])
            .output();

        match result {
            Ok(out) if out.status.success() => {
                // Decompress .zst if needed
                if obj.key.ends_with(".zst") {
                    let compressed = std::fs::read(&dest_path)?;
                    let decompressed = zstd::decode_all(&compressed[..])?;
                    let uncompressed_path = dest_path.with_extension("");
                    std::fs::write(&uncompressed_path, decompressed)?;
                    std::fs::remove_file(&dest_path)?;
                    tracing::info!(key = %obj.key, dest = %uncompressed_path.display(), "decompressed");
                }
                restored += 1;
            }
            Ok(out) => {
                errors += 1;
                let stderr = String::from_utf8_lossy(&out.stderr);
                tracing::error!(key = %obj.key, error = %stderr.trim(), "restore failed");
            }
            Err(e) => {
                errors += 1;
                tracing::error!(key = %obj.key, error = %e, "mc cp command failed");
            }
        }
    }

    println!(
        "Restore complete: {} restored, {} skipped, {} errors ({} total objects)",
        restored,
        skipped,
        errors,
        objects.len()
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
        if key.is_empty() {
            continue;
        }

        // mc ls returns key relative to the listed path.
        // e.g. listing `guser/bucket/backup/` returns key="mydb.db"
        let local_path = PathBuf::from(&key);

        objects.push(RestoreObject { key, local_path });
    }

    Ok(objects)
}

/// Guess the mc alias from an S3 endpoint URL.
fn infer_alias(endpoint: &str) -> Option<&str> {
    if endpoint.contains("127.0.0.1") || endpoint.contains("localhost") {
        Some("guser")
    } else if endpoint.contains("amazonaws.com") {
        Some("s3")
    } else {
        None
    }
}
