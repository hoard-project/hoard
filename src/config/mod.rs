#![allow(dead_code)]
//! Configuration loading: v1 compat, v2 StorageClass/Volume, CLI override.
//!
//! ## Load order
//!
//! 1. TOML file (v1 or v2, detected by `[hoard].version`)
//! 2. conf.d/ directories (v2 only)
//! 3. CLI flags / env vars (highest priority)
//!
//! ## v1 → v2 auto-upgrade
//!
//! v1 configs are translated to a single default volume so existing
//! deployments keep working without changes.
#![deny(unsafe_code)]

mod compat;
pub mod env;
pub mod registry;
pub mod v2;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use url::Url;

pub use compat::ConfigFile;
pub use v2::ResolvedVolume;

// ── CLI layer ────────────────────────────────────────────────────

/// Hoard: eBPF + io_uring zero-copy SQLite backup to S3.
#[derive(Parser, Debug, Clone)]
#[command(name = "hoard", version, about)]
pub struct Config {
    // ── Config file ──
    /// Path to TOML configuration file (CLI flags override file values)
    #[arg(long, env = "HOARD_CONFIG")]
    pub config: Option<PathBuf>,

    /// Deployment mode
    #[arg(long, env = "HOARD_MODE")]
    pub mode: Option<Mode>,

    // ── Service identification ──
    /// Logical service name (standalone mode)
    #[arg(long, env = "HOARD_SERVICE")]
    pub service: Option<String>,

    // ── File watching ──
    /// Root directory to monitor (canonicalized at startup)
    #[arg(long, env = "HOARD_WATCH_PATH")]
    pub watch_path: Option<PathBuf>,

    /// Root directory for Nomad volumes
    #[arg(long, env = "HOARD_WATCH_ROOT")]
    pub watch_root: Option<PathBuf>,

    /// Glob patterns for files to monitor (comma-separated)
    #[arg(long, env = "HOARD_WATCH_PATTERNS")]
    pub watch_patterns: Option<String>,

    /// Glob patterns for files to exclude (comma-separated)
    #[arg(long, env = "HOARD_WATCH_EXCLUDES")]
    pub watch_excludes: Option<String>,

    // ── Transport ──
    /// TLS mode: ktls, plain, userspace
    #[arg(long, env = "HOARD_TLS_MODE")]
    pub tls_mode: Option<TlsModeArg>,

    // ── S3 backend ──
    /// S3 endpoint URL
    #[arg(long, env = "HOARD_S3_ENDPOINT")]
    pub s3_endpoint: Option<Url>,

    /// S3 region
    #[arg(long, env = "HOARD_S3_REGION")]
    pub s3_region: Option<String>,

    /// S3 bucket name
    #[arg(long, env = "HOARD_S3_BUCKET")]
    pub s3_bucket: Option<String>,

    /// S3 access key (env only — never pass on command line)
    #[arg(long, env = "HOARD_S3_ACCESS_KEY", hide_env_values = true)]
    pub s3_access_key: Option<String>,

    /// S3 secret key (env only — never pass on command line)
    #[arg(long, env = "HOARD_S3_SECRET_KEY", hide_env_values = true)]
    pub s3_secret_key: Option<String>,

    /// Skip SigV4 signing (for local S3 / anonymous bucket access)
    #[arg(long, env = "HOARD_S3_NO_SIGN")]
    pub s3_no_sign: Option<bool>,

    /// S3 key prefix for backups
    #[arg(long, env = "HOARD_S3_PREFIX")]
    pub s3_prefix: Option<String>,

    // ── GC ──
    /// Garbage collection interval in seconds
    #[arg(long, env = "HOARD_GC_INTERVAL")]
    pub gc_interval: Option<u64>,

    /// TTL for backup objects before GC deletes them
    #[arg(long, env = "HOARD_GC_TTL_DAYS")]
    pub gc_ttl_days: Option<u32>,

    // ── Resilience ──
    /// Path to SQLite database for pending-set persistence
    #[arg(long, env = "HOARD_PENDING_DB")]
    pub pending_db: Option<String>,

    /// Maximum upload retry attempts per file
    #[arg(long, env = "HOARD_MAX_UPLOAD_RETRIES")]
    pub max_upload_retries: Option<u32>,

    /// Directory for dead-letter queue (failed uploads after max retries)
    #[arg(long, env = "HOARD_DEAD_LETTER_DIR")]
    pub dead_letter_dir: Option<String>,

    // ── Nomad integration ──
    /// Nomad agent address
    #[arg(long, env = "HOARD_NOMAD_ADDR")]
    pub nomad_addr: Option<String>,

    /// Nomad ACL token
    #[arg(long, env = "HOARD_NOMAD_TOKEN", hide_env_values = true)]
    pub nomad_token: Option<String>,

    // ── Control socket ──
    /// Path for the hoardctl control socket
    #[arg(long, env = "HOARD_CONTROL_SOCKET")]
    pub control_socket: Option<PathBuf>,

    // ── Metrics ──
    /// Metrics listen address
    #[arg(long, env = "HOARD_METRICS_ADDR")]
    pub metrics_addr: Option<String>,

    // ── Internal ──
    /// Saved config path for SIGHUP reload
    #[arg(skip)]
    pub config_path: Option<PathBuf>,
}

/// Deployment mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    Standalone,
    Nomad,
}

/// TLS mode from CLI argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TlsModeArg {
    Ktls,
    Plain,
    Userspace,
}

// ── Validated config ─────────────────────────────────────────────

/// Fully validated, canonicalized runtime configuration.
///
/// This is what `hoard.rs` consumes.  It supports both v1 (legacy flat)
/// and v2 (StorageClass/Volume model) configs.
#[derive(Debug, Clone)]
pub struct ValidatedConfig {
    pub mode: Mode,
    pub service: String,
    pub watch_path: PathBuf,
    pub watch_patterns: Vec<String>,
    pub watch_excludes: Vec<String>,
    #[allow(dead_code)]
    pub tls_mode: TlsModeArg,
    pub s3_endpoint: Url,
    pub s3_region: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    #[allow(dead_code)]
    pub s3_prefix: String,
    pub s3_no_sign: bool,
    pub gc_interval_secs: u64,
    #[allow(dead_code)]
    pub gc_ttl_days: u32,
    pub nomad_addr: Option<String>,
    pub nomad_token: Option<String>,
    pub control_socket: PathBuf,
    pub metrics_addr: String,
    pub pending_db: PathBuf,
    pub max_upload_retries: u32,
    pub dead_letter_dir: PathBuf,
    #[allow(dead_code)]
    pub config_path: Option<PathBuf>,

    // ── v2 additions ──
    /// Resolved volumes (v2 only; v1 generates a single default volume).
    pub volumes: Vec<ResolvedVolume>,
    /// Nomad meta auto-discovery enabled.
    pub nomad_meta_enabled: bool,
    /// Nomad meta poll interval in seconds.
    pub nomad_meta_poll_secs: u64,
}

/// Convert LegacyConfig + resolved volumes into ValidatedConfig.
fn legacy_to_validated(lc: compat::LegacyConfig, volumes: Vec<ResolvedVolume>) -> ValidatedConfig {
    ValidatedConfig {
        mode: match lc.mode {
            compat::Mode::Standalone => Mode::Standalone,
            compat::Mode::Nomad => Mode::Nomad,
        },
        service: lc.service,
        watch_path: lc.watch_path,
        watch_patterns: lc.watch_patterns,
        watch_excludes: lc.watch_excludes,
        tls_mode: match lc.tls_mode {
            compat::TlsMode::Ktls => TlsModeArg::Ktls,
            compat::TlsMode::Plain => TlsModeArg::Plain,
            compat::TlsMode::Userspace => TlsModeArg::Userspace,
        },
        s3_endpoint: lc.s3_endpoint,
        s3_region: lc.s3_region,
        s3_bucket: lc.s3_bucket,
        s3_access_key: lc.s3_access_key,
        s3_secret_key: lc.s3_secret_key,
        s3_prefix: lc.s3_prefix,
        s3_no_sign: lc.s3_no_sign,
        gc_interval_secs: lc.gc_interval_secs,
        gc_ttl_days: lc.gc_ttl_days,
        nomad_addr: lc.nomad_addr,
        nomad_token: lc.nomad_token,
        control_socket: lc.control_socket,
        metrics_addr: lc.metrics_addr,
        pending_db: lc.pending_db,
        max_upload_retries: lc.max_upload_retries,
        dead_letter_dir: lc.dead_letter_dir,
        config_path: lc.config_path,
        volumes,
        nomad_meta_enabled: lc.nomad_meta_enabled,
        nomad_meta_poll_secs: lc.nomad_meta_poll_secs,
    }
}

// ── Loading ──────────────────────────────────────────────────────

impl Config {
    /// Load and validate the full configuration.
    pub fn load(self) -> Result<ValidatedConfig> {
        // ── Step 1: Load TOML (v1 or v2) ──
        let (legacy, volumes) = if let Some(ref config_path) = self.config {
            let raw = std::fs::read_to_string(config_path)
                .with_context(|| format!("reading config: {}", config_path.display()))?;

            // Detect version
            let is_v2 = toml::from_str::<toml::Table>(&raw)
                .ok()
                .and_then(|t| {
                    t.get("hoard")
                        .and_then(toml::Value::as_table)
                        .and_then(|h| h.get("version"))
                        .and_then(toml::Value::as_integer)
                })
                .map(|v| v == 2)
                .unwrap_or(false);

            tracing::info!(is_v2, config_path = %config_path.display(), "config version detection");

            match is_v2 {
                true => {
                    let v2 = v2::load(config_path)?;
                    let volumes = v2::resolve_volumes(&v2)?;
                    tracing::info!(
                        "loaded v2 config from {}: {} storage_classes, {} volumes",
                        config_path.display(),
                        v2.storage_classes.len(),
                        volumes.len()
                    );
                    for v in &volumes {
                        tracing::info!(
                            "  volume '{}': match={}, s3_prefix={}, ttl={}",
                            v.name,
                            v.match_glob,
                            v.s3_prefix,
                            v.ttl
                        );
                    }
                    (compat::v2_to_legacy(&v2, config_path), volumes)
                }
                false => {
                    // v1 or unknown → treat as v1
                    tracing::info!("loading v1 config from {}", config_path.display());
                    compat::load_v1_with_default_volume(config_path)?
                }
            }
        } else {
            // No config file — use defaults
            (compat::default_legacy(), compat::default_single_volume())
        };

        // ── Step 2: Apply CLI overrides ──
        let mut legacy = legacy;
        self.apply_overrides(&mut legacy);

        // ── Step 3: Validate ──
        if legacy.s3_endpoint.as_str().is_empty() {
            anyhow::bail!("S3 endpoint is required");
        }
        if legacy.s3_bucket.is_empty() {
            anyhow::bail!("S3 bucket is required");
        }

        Ok(legacy_to_validated(legacy, volumes))
    }

    fn apply_overrides(&self, vc: &mut compat::LegacyConfig) {
        if let Some(ref m) = self.mode {
            vc.mode = match m {
                Mode::Standalone => compat::Mode::Standalone,
                Mode::Nomad => compat::Mode::Nomad,
            };
        }
        if let Some(ref s) = self.service {
            vc.service = s.clone();
        }
        if let Some(ref p) = self.watch_path {
            vc.watch_path = p.clone();
        }
        if let Some(ref p) = self.watch_patterns {
            vc.watch_patterns = p.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Some(ref e) = self.watch_excludes {
            vc.watch_excludes = e.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Some(ref e) = self.s3_endpoint {
            vc.s3_endpoint = e.clone();
        }
        if let Some(ref r) = self.s3_region {
            vc.s3_region = r.clone();
        }
        if let Some(ref b) = self.s3_bucket {
            vc.s3_bucket = b.clone();
        }
        if let Some(ref k) = self.s3_access_key {
            vc.s3_access_key = k.clone();
        }
        if let Some(ref s) = self.s3_secret_key {
            vc.s3_secret_key = s.clone();
        }
        if let Some(ref p) = self.s3_prefix {
            vc.s3_prefix = p.clone();
        }
        if let Some(n) = self.s3_no_sign {
            vc.s3_no_sign = n;
        }
        if let Some(i) = self.gc_interval {
            vc.gc_interval_secs = i;
        }
        if let Some(t) = self.gc_ttl_days {
            vc.gc_ttl_days = t;
        }
        if let Some(ref a) = self.nomad_addr {
            vc.nomad_addr = Some(a.clone());
        }
        if let Some(ref t) = self.nomad_token {
            vc.nomad_token = Some(t.clone());
        }
        if let Some(ref s) = self.control_socket {
            vc.control_socket = s.clone();
        }
        if let Some(ref a) = self.metrics_addr {
            vc.metrics_addr = a.clone();
        }
        if let Some(ref p) = self.pending_db {
            vc.pending_db = PathBuf::from(p);
        }
        if let Some(r) = self.max_upload_retries {
            vc.max_upload_retries = r;
        }
        if let Some(ref d) = self.dead_letter_dir {
            vc.dead_letter_dir = PathBuf::from(d);
        }
        if let Some(ref t) = self.tls_mode {
            vc.tls_mode = match t {
                TlsModeArg::Ktls => compat::TlsMode::Ktls,
                TlsModeArg::Plain => compat::TlsMode::Plain,
                TlsModeArg::Userspace => compat::TlsMode::Userspace,
            };
        }
        // watch_root overrides watch_path
        if let Some(ref root) = self.watch_root {
            vc.watch_path = root.clone();
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v1_backward_compat() {
        // Simulate old config path
        let vc = compat::default_legacy();
        assert_eq!(vc.mode, compat::Mode::Standalone);
        assert!(!vc.s3_bucket.is_empty());
    }
}
