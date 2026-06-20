//! Configuration loading: v2 StorageClass/Volume model + CLI override.
//!
//! ## Load order
//!
//! 1. v2 TOML file (detected by `[hoard].version = 2`)
//! 2. conf.d/ directories
//! 3. CLI flags / env vars (highest priority)
#![deny(unsafe_code)]

pub mod env;
mod raw;
pub mod registry;
pub mod v2;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use url::Url;

pub use v2::ResolvedVolume;

// ── CLI layer ────────────────────────────────────────────────────

/// Hoard: eBPF + io_uring zero-copy file backup to S3.
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

    /// Enable Nomad meta auto-discovery (poll /v1/jobs for hoard_enabled volumes)
    #[arg(long, env = "HOARD_NOMAD_META_ENABLED")]
    pub nomad_meta_enabled: Option<bool>,

    /// Nomad meta poll interval in seconds
    #[arg(long, env = "HOARD_NOMAD_META_POLL_SECS")]
    pub nomad_meta_poll_secs: Option<u64>,

    // ── Control socket ──
    /// Path for the hoardctl control socket
    #[arg(long, env = "HOARD_CONTROL_SOCKET")]
    pub control_socket: Option<PathBuf>,

    // ── Metrics ──
    /// Metrics listen address
    #[arg(long, env = "HOARD_METRICS_ADDR")]
    pub metrics_addr: Option<String>,
    // ── Internal ──
}

/// Deployment mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Mode {
    /// Standalone: single-machine deployment
    Standalone,
    /// Nomad: cluster deployment with meta auto-discovery
    Nomad,
}

/// TLS mode from CLI argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum TlsModeArg {
    /// Kernel TLS (requires 5.5+)
    Ktls,
    /// Plain TCP (no encryption)
    Plain,
    /// Userspace TLS (rustls)
    Userspace,
}

// ── Validated config ─────────────────────────────────────────────

/// Fully validated configuration, ready for daemon startup.
#[derive(Debug, Clone)]
pub struct ValidatedConfig {
    pub mode: Mode,
    pub service: String,
    pub watch_path: PathBuf,
    pub watch_patterns: Vec<String>,
    pub watch_excludes: Vec<String>,
    pub s3_endpoint: Url,
    pub s3_region: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_no_sign: bool,
    pub gc_interval_secs: u64,
    pub nomad_addr: Option<String>,
    pub nomad_token: Option<String>,
    pub control_socket: PathBuf,
    pub metrics_addr: String,
    pub pending_db: PathBuf,
    pub max_upload_retries: u32,
    pub dead_letter_dir: PathBuf,
    pub config_path: Option<PathBuf>,
    /// Resolved volumes (from v2 TOML + conf.d).
    pub volumes: Vec<ResolvedVolume>,
    /// Nomad meta auto-discovery enabled.
    pub nomad_meta_enabled: bool,
    /// Nomad meta poll interval in seconds.
    pub nomad_meta_poll_secs: u64,
}

/// Convert RawConfig + resolved volumes into ValidatedConfig.
fn raw_to_validated(raw: raw::RawConfig, volumes: Vec<ResolvedVolume>) -> ValidatedConfig {
    ValidatedConfig {
        mode: raw.mode,
        service: raw.service,
        watch_path: raw.watch_path,
        watch_patterns: raw.watch_patterns,
        watch_excludes: raw.watch_excludes,
        s3_endpoint: raw.s3_endpoint,
        s3_region: raw.s3_region,
        s3_bucket: raw.s3_bucket,
        s3_access_key: raw.s3_access_key,
        s3_secret_key: raw.s3_secret_key,
        s3_no_sign: raw.s3_no_sign,
        gc_interval_secs: raw.gc_interval_secs,
        nomad_addr: raw.nomad_addr,
        nomad_token: raw.nomad_token,
        control_socket: raw.control_socket,
        metrics_addr: raw.metrics_addr,
        pending_db: raw.pending_db,
        max_upload_retries: raw.max_upload_retries,
        dead_letter_dir: raw.dead_letter_dir,
        config_path: raw.config_path,
        volumes,
        nomad_meta_enabled: raw.nomad_meta_enabled,
        nomad_meta_poll_secs: raw.nomad_meta_poll_secs,
    }
}

// ── Loading ──────────────────────────────────────────────────────

impl Config {
    /// Load and validate the full configuration.
    pub fn load(self) -> Result<ValidatedConfig> {
        let (raw_cfg, volumes) = if let Some(ref config_path) = self.config {
            let raw = std::fs::read_to_string(config_path)
                .with_context(|| format!("reading config: {}", config_path.display()))?;

            let v2_cfg: v2::ConfigV2 = toml::from_str(&raw)
                .with_context(|| format!("parsing v2 config: {}", config_path.display()))?;

            tracing::info!(config_path = %config_path.display(), "loaded v2 config");

            let volumes = v2::resolve_volumes(&v2_cfg)?;
            tracing::info!(
                "{} storage_classes, {} volumes",
                v2_cfg.storage_classes.len(),
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
            (raw::v2_to_raw(&v2_cfg, config_path), volumes)
        } else {
            (raw::default_raw(), raw::default_single_volume())
        };

        // Apply CLI overrides on top of config file defaults
        let mut raw_cfg = raw_cfg;
        self.apply_overrides(&mut raw_cfg);

        // Validate
        if raw_cfg.s3_endpoint.as_str().is_empty() {
            anyhow::bail!("S3 endpoint is required");
        }
        if raw_cfg.s3_bucket.is_empty() {
            anyhow::bail!("S3 bucket is required");
        }

        Ok(raw_to_validated(raw_cfg, volumes))
    }

    fn apply_overrides(&self, raw: &mut raw::RawConfig) {
        if let Some(ref m) = self.mode {
            raw.mode = *m;
        }
        if let Some(ref s) = self.service {
            raw.service = s.clone();
        }
        if let Some(ref p) = self.watch_path {
            raw.watch_path = p.clone();
        }
        if let Some(ref roots) = self.watch_root {
            raw.watch_path = roots.clone();
        }
        if let Some(ref pats) = self.watch_patterns {
            raw.watch_patterns = pats.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Some(ref ex) = self.watch_excludes {
            raw.watch_excludes = ex.split(',').map(|s| s.trim().to_string()).collect();
        }
        if let Some(ref e) = self.s3_endpoint {
            raw.s3_endpoint = e.clone();
        }
        if let Some(ref r) = self.s3_region {
            raw.s3_region = r.clone();
        }
        if let Some(ref b) = self.s3_bucket {
            raw.s3_bucket = b.clone();
        }
        if let Some(ref k) = self.s3_access_key {
            raw.s3_access_key = k.clone();
        }
        if let Some(ref s) = self.s3_secret_key {
            raw.s3_secret_key = s.clone();
        }
        if let Some(ref p) = self.s3_prefix {
            raw.s3_prefix = p.clone();
        }
        if let Some(n) = self.s3_no_sign {
            raw.s3_no_sign = n;
        }
        if let Some(i) = self.gc_interval {
            raw.gc_interval_secs = i;
        }
        if let Some(t) = self.gc_ttl_days {
            raw.gc_ttl_days = t;
        }
        if let Some(ref a) = self.nomad_addr {
            raw.nomad_addr = Some(a.clone());
        }
        if let Some(ref t) = self.nomad_token {
            raw.nomad_token = Some(t.clone());
        }
        if let Some(e) = self.nomad_meta_enabled {
            raw.nomad_meta_enabled = e;
        }
        if let Some(p) = self.nomad_meta_poll_secs {
            raw.nomad_meta_poll_secs = p;
        }
        if let Some(ref s) = self.control_socket {
            raw.control_socket = s.clone();
        }
        if let Some(ref a) = self.metrics_addr {
            raw.metrics_addr = a.clone();
        }
        if let Some(ref p) = self.pending_db {
            raw.pending_db = PathBuf::from(p);
        }
        if let Some(r) = self.max_upload_retries {
            raw.max_upload_retries = r;
        }
        if let Some(ref d) = self.dead_letter_dir {
            raw.dead_letter_dir = PathBuf::from(d);
        }
        if let Some(ref t) = self.tls_mode {
            raw.tls_mode = *t;
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_defaults_are_standalone() {
        let raw = raw::default_raw();
        assert_eq!(raw.mode, Mode::Standalone);
    }

    #[test]
    fn raw_defaults_single_volume() {
        let volumes = raw::default_single_volume();
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].name, "default");
    }

    #[test]
    fn config_default_has_default_service() {
        let vc = ValidatedConfig {
            mode: Mode::Standalone,
            service: "default".into(),
            watch_path: PathBuf::from("/"),
            watch_patterns: vec![],
            watch_excludes: vec![],
            s3_endpoint: Url::parse("http://localhost:9000").unwrap(),
            s3_region: "us-east-1".into(),
            s3_bucket: "test".into(),
            s3_access_key: String::new(),
            s3_secret_key: String::new(),
            s3_no_sign: false,
            gc_interval_secs: 3600,
            nomad_addr: None,
            nomad_token: None,
            control_socket: PathBuf::from("/run/hoard.sock"),
            metrics_addr: "0.0.0.0:9090".into(),
            pending_db: PathBuf::from("/tmp/pending.db"),
            max_upload_retries: 5,
            dead_letter_dir: PathBuf::from("/tmp/dead"),
            config_path: None,
            volumes: vec![],
            nomad_meta_enabled: false,
            nomad_meta_poll_secs: 300,
        };
        assert_eq!(vc.service, "default");
    }
}
