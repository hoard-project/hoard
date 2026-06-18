//! Configuration parsing and validation.
//!
//! Supports two layers:
//! 1. **TOML config file** (`--config` / `HOARD_CONFIG`) — base values
//! 2. **CLI flags + env vars** — override TOML values
//!
//! When no `--config` is given, behaves identically to the old CLI-only path.
//!
//! TOML values may contain `${ENV_VAR}` placeholders, expanded at load time.

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use clap::Parser;
use serde::Deserialize;
use std::path::PathBuf;
use url::Url;

// ── TOML config file schema ──────────────────────────────────────

/// Hoard TOML configuration file schema.
///
/// All fields are optional — CLI flags / env vars provide defaults
/// or override.  String values support `${ENV}` expansion.
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    #[serde(default)]
    pub watch: WatchSection,
    #[serde(default)]
    pub s3: S3Section,
    #[serde(default)]
    pub nomad: NomadSection,
    #[serde(default)]
    pub gc: GcSection,
    #[serde(default)]
    pub filter: FilterSection,
    #[serde(default)]
    pub daemon: DaemonSection,
    #[serde(default)]
    pub resilience: ResilienceSection,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResilienceSection {
    pub pending_db: Option<String>,
    pub max_upload_retries: Option<u32>,
    pub dead_letter_dir: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct WatchSection {
    pub path: Option<String>,
    pub patterns: Option<String>,
    pub excludes: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct S3Section {
    pub endpoint: Option<String>,
    pub region: Option<String>,
    pub bucket: Option<String>,
    pub access_key: Option<String>,
    pub secret_key: Option<String>,
    pub no_sign: Option<bool>,
    pub prefix: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct NomadSection {
    pub addr: Option<String>,
    pub token: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct GcSection {
    pub interval_secs: Option<u64>,
    pub ttl_days: Option<u32>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct FilterSection {
    pub extensions: Option<Vec<String>>,
    pub exclude: Option<Vec<String>>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct DaemonSection {
    pub mode: Option<String>,
    pub service: Option<String>,
    pub control_socket: Option<String>,
    pub metrics_addr: Option<String>,
}

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

    /// Skip SigV4 signing (for MinIO / anonymous bucket access)
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
    /// Prometheus metrics listen address
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

/// Validated configuration — all paths canonicalized, all required fields present.
#[derive(Debug, Clone)]
pub struct ValidatedConfig {
    pub mode: Mode,
    pub service: String,
    pub watch_path: PathBuf,
    pub watch_patterns: Vec<String>,
    pub watch_excludes: Vec<String>,
    pub tls_mode: TlsModeArg,
    pub s3_endpoint: Url,
    pub s3_region: String,
    pub s3_bucket: String,
    pub s3_access_key: String,
    pub s3_secret_key: String,
    pub s3_prefix: String,
    pub s3_no_sign: bool,
    pub gc_interval_secs: u64,
    pub gc_ttl_days: u32,
    pub nomad_addr: Option<String>,
    pub nomad_token: Option<String>,
    pub control_socket: PathBuf,
    pub metrics_addr: String,
    /// Path to SQLite database for pending-set persistence
    pub pending_db: PathBuf,
    /// Maximum upload retry attempts per file
    pub max_upload_retries: u32,
    /// Directory for dead-letter queue
    pub dead_letter_dir: PathBuf,
    /// Path to the TOML config file (if loaded from file), for SIGHUP reload.
    pub config_path: Option<PathBuf>,
}

// ── TOML loading + env expansion ─────────────────────────────────

/// Expand `${ENV_VAR}` placeholders in a string.
fn expand_env(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '$' && chars.peek() == Some(&'{') {
            chars.next(); // skip '{'
            let mut var = String::new();
            for ch in chars.by_ref() {
                if ch == '}' {
                    break;
                }
                var.push(ch);
            }
            if let Ok(val) = std::env::var(&var) {
                result.push_str(&val);
            }
        } else {
            result.push(c);
        }
    }
    result
}

impl ConfigFile {
    /// Load and parse a TOML config file, expanding `${ENV}` placeholders.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config: {}", path.display()))?;
        let expanded = expand_env(&raw);
        let cfg: Self = toml::from_str(&expanded)
            .with_context(|| format!("invalid TOML in {}", path.display()))?;
        Ok(cfg)
    }

    /// Convert filter `extensions` to a watch-patterns string.
    fn filter_patterns(&self) -> Option<String> {
        self.filter.extensions.as_ref().map(|exts| {
            exts.iter()
                .map(|e| format!("*.{e}"))
                .collect::<Vec<_>>()
                .join(",")
        })
    }

    /// Convert filter `exclude` to a watch-excludes string.
    fn filter_excludes(&self) -> Option<String> {
        self.filter.exclude.as_ref().map(|v| v.join(","))
    }
}

// ── Config merge ─────────────────────────────────────────────────

impl Config {
    /// Load configuration: TOML file first, CLI/env overrides second.
    pub fn load() -> Result<Self> {
        let cli = Self::parse();

        // If no config file, return CLI as-is
        let Some(cfg_path) = cli.config.clone() else {
            return Ok(cli);
        };

        let file = ConfigFile::load(&cfg_path)?;

        let mut merged = cli.merge_file(&file);
        merged.config_path = Some(cfg_path);
        Ok(merged)
    }

    /// Merge TOML file values into CLI config.
    /// CLI values (non-None) take precedence over file values.
    fn merge_file(mut self, f: &ConfigFile) -> Self {
        // Daemon
        if self.mode.is_none() {
            if let Some(ref s) = f.daemon.mode {
                self.mode = Some(match s.to_lowercase().as_str() {
                    "nomad" => Mode::Nomad,
                    _ => Mode::Standalone,
                });
            }
        }
        if self.service.is_none() {
            self.service = f.daemon.service.clone();
        }
        if self.control_socket.is_none() {
            self.control_socket = f.daemon.control_socket.as_deref().map(PathBuf::from);
        }
        if self.metrics_addr.is_none() {
            self.metrics_addr = f.daemon.metrics_addr.clone();
        }

        // Watch
        if self.watch_path.is_none() {
            if let Some(ref p) = f.watch.path {
                self.watch_path = Some(PathBuf::from(p));
            }
        }
        if self.watch_patterns.is_none() {
            self.watch_patterns = f.filter_patterns();
        }
        if self.watch_excludes.is_none() {
            self.watch_excludes = f.filter_excludes();
        }

        // S3
        if self.s3_endpoint.is_none() {
            if let Some(ref ep) = f.s3.endpoint {
                self.s3_endpoint = ep.parse().ok();
            }
        }
        if self.s3_region.is_none() {
            self.s3_region = f.s3.region.clone();
        }
        if self.s3_bucket.is_none() {
            self.s3_bucket = f.s3.bucket.clone();
        }
        if self.s3_access_key.is_none() {
            self.s3_access_key = f.s3.access_key.clone();
        }
        if self.s3_secret_key.is_none() {
            self.s3_secret_key = f.s3.secret_key.clone();
        }
        if self.s3_no_sign.is_none() {
            self.s3_no_sign = f.s3.no_sign;
        }
        if self.s3_prefix.is_none() {
            self.s3_prefix = f.s3.prefix.clone();
        }

        // Nomad
        if self.nomad_addr.is_none() {
            self.nomad_addr = f.nomad.addr.clone();
        }
        if self.nomad_token.is_none() {
            self.nomad_token = f.nomad.token.clone();
        }

        // GC
        if self.gc_interval.is_none() {
            self.gc_interval = f.gc.interval_secs;
        }
        if self.gc_ttl_days.is_none() {
            self.gc_ttl_days = f.gc.ttl_days;
        }

        // Resilience
        if self.pending_db.is_none() {
            self.pending_db = f.resilience.pending_db.clone();
        }
        if self.max_upload_retries.is_none() {
            self.max_upload_retries = f.resilience.max_upload_retries;
        }
        if self.dead_letter_dir.is_none() {
            self.dead_letter_dir = f.resilience.dead_letter_dir.clone();
        }

        self
    }

    /// Validate and canonicalize the configuration.
    pub fn validate(self) -> Result<ValidatedConfig> {
        let mode = self.mode.unwrap_or(Mode::Standalone);
        let service = self.service.unwrap_or_else(|| "default".to_string());

        // Watch path — canonicalize
        let watch_path = match self.watch_path {
            Some(p) => std::fs::canonicalize(&p)
                .with_context(|| format!("watch-path does not exist: {}", p.display()))?,
            None => self
                .watch_root
                .clone()
                .unwrap_or_else(|| PathBuf::from("/var/lib/hoard/volumes")),
        };

        let watch_patterns: Vec<String> = self
            .watch_patterns
            .unwrap_or_else(|| "*.db,*.db-wal,*.db-shm".to_string())
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let watch_excludes: Vec<String> = self
            .watch_excludes
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let s3_endpoint: Url = self
            .s3_endpoint
            .or_else(|| "http://127.0.0.1:9000".parse().ok())
            .context("--s3-endpoint is required")?;

        let s3_bucket = self.s3_bucket.context("--s3-bucket is required")?;
        let s3_no_sign = self.s3_no_sign.unwrap_or(false);

        let s3_access_key = if s3_no_sign {
            self.s3_access_key.unwrap_or_else(|| "dummy".to_string())
        } else {
            self.s3_access_key
                .context("--s3-access-key (or HOARD_S3_ACCESS_KEY) is required")?
        };

        let s3_secret_key = if s3_no_sign {
            self.s3_secret_key.unwrap_or_else(|| "dummy".to_string())
        } else {
            self.s3_secret_key
                .context("--s3-secret-key (or HOARD_S3_SECRET_KEY) is required")?
        };

        let control_socket = self
            .control_socket
            .unwrap_or_else(|| PathBuf::from(format!("/run/hoard/{service}.sock")));

        Ok(ValidatedConfig {
            mode,
            service,
            watch_path,
            watch_patterns,
            watch_excludes,
            tls_mode: self.tls_mode.unwrap_or(TlsModeArg::Ktls),
            s3_endpoint,
            s3_region: self.s3_region.unwrap_or_else(|| "us-east-1".to_string()),
            s3_bucket,
            s3_access_key,
            s3_secret_key,
            s3_prefix: self.s3_prefix.unwrap_or_else(|| "backup".to_string()),
            s3_no_sign,
            gc_interval_secs: self.gc_interval.unwrap_or(21600),
            gc_ttl_days: self.gc_ttl_days.unwrap_or(7),
            nomad_addr: self.nomad_addr,
            nomad_token: self.nomad_token,
            control_socket,
            metrics_addr: self
                .metrics_addr
                .unwrap_or_else(|| "127.0.0.1:9150".to_string()),
            pending_db: self
                .pending_db
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/var/lib/hoard/pending.db")),
            max_upload_retries: self.max_upload_retries.unwrap_or(5),
            dead_letter_dir: self
                .dead_letter_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/var/lib/hoard/dead-letter")),
            config_path: self.config_path.clone(),
        })
    }
}
