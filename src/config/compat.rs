//! v1 backward compatibility layer.
//!
//! v1 flat configs are loaded and mapped to the same `LegacyConfig`
//! struct that ValidatedConfig wraps.  CLI overrides work identically
//! for both v1 and v2 sources.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use url::Url;

use super::env;

// ── Legacy config (shared by v1 loading and v2→legacy mapping) ──

#[derive(Debug, Clone)]
pub struct LegacyConfig {
    pub mode: Mode,
    pub service: String,
    pub watch_path: PathBuf,
    pub watch_patterns: Vec<String>,
    pub watch_excludes: Vec<String>,
    pub tls_mode: TlsMode,
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
    pub pending_db: PathBuf,
    pub max_upload_retries: u32,
    pub dead_letter_dir: PathBuf,
    pub config_path: Option<PathBuf>,
    pub nomad_meta_enabled: bool,
    pub nomad_meta_poll_secs: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Standalone,
    Nomad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    Ktls,
    Plain,
    Userspace,
}

// ── Defaults ─────────────────────────────────────────────────────

pub fn default_legacy() -> LegacyConfig {
    LegacyConfig {
        mode: Mode::Standalone,
        service: "default".to_string(),
        watch_path: PathBuf::from("/var/lib/hoard/volumes"),
        watch_patterns: vec!["*".to_string()],
        watch_excludes: vec!["*.tmp".to_string(), "*.journal".to_string()],
        tls_mode: TlsMode::Plain,
        s3_endpoint: Url::parse("http://localhost:9000").unwrap(),
        s3_region: "us-east-1".to_string(),
        s3_bucket: "backups".to_string(),
        s3_access_key: String::new(),
        s3_secret_key: String::new(),
        s3_prefix: "default".to_string(),
        s3_no_sign: false,
        gc_interval_secs: 3600,
        gc_ttl_days: 30,
        nomad_addr: None,
        nomad_token: None,
        control_socket: PathBuf::from("/var/run/hoard.sock"),
        metrics_addr: "0.0.0.0:9150".to_string(),
        pending_db: PathBuf::from("/var/lib/hoard/pending.db"),
        max_upload_retries: 5,
        dead_letter_dir: PathBuf::from("/var/lib/hoard/dead-letter"),
        config_path: None,
        nomad_meta_enabled: false,
        nomad_meta_poll_secs: 300,
    }
}

// ── v1 TOML schema ───────────────────────────────────────────────

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigFileV1 {
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
    pub resilience: ResilienceSectionV1,
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

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResilienceSectionV1 {
    pub pending_db: Option<String>,
    pub max_upload_retries: Option<u32>,
    pub dead_letter_dir: Option<String>,
}

// ── Loaders ──────────────────────────────────────────────────────

/// v1 TOML config file (re-exported for SIGHUP reload backward compat).
pub use ConfigFileV1 as ConfigFile;

impl ConfigFileV1 {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading v1 config: {}", path.display()))?;
        let expanded = env::expand_env(&raw);
        toml::from_str(&expanded)
            .with_context(|| format!("parsing v1 config: {}", path.display()))
    }
}

/// Load a v1 TOML config file.
pub fn load_v1(path: &Path) -> Result<LegacyConfig> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading v1 config: {}", path.display()))?;
    let expanded = env::expand_env(&raw);
    let file: ConfigFileV1 = toml::from_str(&expanded)
        .with_context(|| format!("parsing v1 config: {}", path.display()))?;

    let mut def = default_legacy();
    def.config_path = Some(path.to_path_buf());

    if let Some(ref m) = file.daemon.mode {
        def.mode = match m.as_str() {
            "nomad" => Mode::Nomad,
            _ => Mode::Standalone,
        };
    }
    if let Some(ref s) = file.daemon.service { def.service = s.clone(); }
    if let Some(ref s) = file.daemon.control_socket { def.control_socket = PathBuf::from(s); }
    if let Some(ref a) = file.daemon.metrics_addr { def.metrics_addr = a.clone(); }

    if let Some(ref p) = file.watch.path { def.watch_path = PathBuf::from(p); }
    if let Some(ref p) = file.watch.patterns {
        def.watch_patterns = p.split(',').map(|s| s.trim().to_string()).collect();
    }
    if let Some(ref e) = file.watch.excludes {
        def.watch_excludes = e.split(',').map(|s| s.trim().to_string()).collect();
    }

    if let Some(ref e) = file.s3.endpoint { def.s3_endpoint = Url::parse(e)?; }
    if let Some(ref r) = file.s3.region { def.s3_region = r.clone(); }
    if let Some(ref b) = file.s3.bucket { def.s3_bucket = b.clone(); }
    if let Some(ref k) = file.s3.access_key { def.s3_access_key = k.clone(); }
    if let Some(ref s) = file.s3.secret_key { def.s3_secret_key = s.clone(); }
    if let Some(p) = file.s3.prefix { def.s3_prefix = p; }
    if let Some(n) = file.s3.no_sign { def.s3_no_sign = n; }

    if let Some(ref a) = file.nomad.addr { def.nomad_addr = Some(a.clone()); }
    if let Some(ref t) = file.nomad.token { def.nomad_token = Some(t.clone()); }

    if let Some(i) = file.gc.interval_secs { def.gc_interval_secs = i; }
    if let Some(t) = file.gc.ttl_days { def.gc_ttl_days = t; }

    if let Some(ref exts) = file.filter.extensions { def.watch_patterns = exts.clone(); }
    if let Some(ref ex) = file.filter.exclude { def.watch_excludes = ex.clone(); }

    if let Some(ref p) = file.resilience.pending_db { def.pending_db = PathBuf::from(p); }
    if let Some(r) = file.resilience.max_upload_retries { def.max_upload_retries = r; }
    if let Some(ref d) = file.resilience.dead_letter_dir { def.dead_letter_dir = PathBuf::from(d); }

    Ok(def)
}

/// Like load_v1 but also returns a default volume for unified config.
pub fn load_v1_with_default_volume(path: &Path) -> Result<(LegacyConfig, Vec<super::v2::ResolvedVolume>)> {
    let legacy = load_v1(path)?;
    let vol = super::v2::ResolvedVolume {
        name: "default".to_string(),
        match_glob: "**".to_string(),
        s3_prefix: legacy.s3_prefix.clone(),
        ttl: format!("{}d", legacy.gc_ttl_days),
        retries: legacy.max_upload_retries,
        extensions: legacy.watch_patterns.clone(),
        exclude: legacy.watch_excludes.clone(),
        compression: None,
        encryption: false,
        on_stop: super::v2::OnStop::Drain,
        on_delete: super::v2::OnDelete::Keep,
    };
    Ok((legacy, vec![vol]))
}

/// Default single volume for no-config mode.
pub fn default_single_volume() -> Vec<super::v2::ResolvedVolume> {
    vec![super::v2::ResolvedVolume {
        name: "default".to_string(),
        match_glob: "**".to_string(),
        s3_prefix: "default".to_string(),
        ttl: "30d".to_string(),
        retries: 5,
        extensions: vec!["*".to_string()],
        exclude: vec!["*.tmp".to_string(), "*.journal".to_string()],
        compression: None,
        encryption: false,
        on_stop: super::v2::OnStop::Drain,
        on_delete: super::v2::OnDelete::Keep,
    }]
}

/// Convert v2 config to the legacy flat format.
///
/// This preserves the v2 semantics (volumes, classes) while providing
/// backward-compatible fields for hoard.rs.
pub fn v2_to_legacy(v2: &super::v2::ConfigV2, config_path: &Path) -> LegacyConfig {
    let mut def = default_legacy();
    def.config_path = Some(config_path.to_path_buf());

    // Daemon
    if let Some(ref m) = v2.daemon.mode {
        def.mode = match m.as_str() {
            "nomad" => Mode::Nomad,
            _ => Mode::Standalone,
        };
    }
    if let Some(ref s) = v2.daemon.service { def.service = s.clone(); }
    if let Some(ref s) = v2.daemon.control_socket { def.control_socket = PathBuf::from(s); }
    if let Some(ref a) = v2.daemon.metrics_addr { def.metrics_addr = a.clone(); }

    // Watch
    if let Some(first) = v2.watch.paths.first() {
        def.watch_path = PathBuf::from(first);
    }

    // S3
    def.s3_endpoint = Url::parse(&v2.s3.endpoint).unwrap_or_else(|_| {
        Url::parse("http://localhost:9000").unwrap()
    });
    def.s3_region = if v2.s3.region.is_empty() { "us-east-1".to_string() } else { v2.s3.region.clone() };
    def.s3_bucket = v2.s3.bucket.clone();
    def.s3_access_key = v2.s3.access_key.clone();
    def.s3_secret_key = v2.s3.secret_key.clone();
    def.s3_no_sign = v2.s3.no_sign;
    // Use the first volume's prefix as global for backward compat
    def.s3_prefix = v2.defaults.prefix.clone().unwrap_or_else(|| "default".to_string());

    // GC (from defaults)
    // Parse "30d" → 30
    def.gc_ttl_days = v2.defaults.ttl.trim_end_matches('d').parse().unwrap_or(30);
    def.gc_interval_secs = 3600; // v2 doesn't have a global GC interval yet

    // Resilience
    if let Some(ref p) = v2.resilience.pending_db { def.pending_db = PathBuf::from(p); }
    if let Some(r) = v2.resilience.max_upload_retries { def.max_upload_retries = r; }
    if let Some(ref d) = v2.resilience.dead_letter_dir { def.dead_letter_dir = PathBuf::from(d); }

    // Nomad
    if let Some(ref a) = v2.nomad.addr { def.nomad_addr = Some(a.clone()); }
    if let Some(ref t) = v2.nomad.token { def.nomad_token = Some(t.clone()); }
    def.nomad_meta_enabled = v2.nomad.meta_enabled;
    def.nomad_meta_poll_secs = v2.nomad.meta_poll_secs.unwrap_or(300);

    def
}
