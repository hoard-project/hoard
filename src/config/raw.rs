//! Raw configuration — intermediate representation between CLI/v2 TOML
//! and the validated `ValidatedConfig`.
//!
//! This replaces the v1 `LegacyConfig` compat layer (removed in v1.0.2).
#![deny(unsafe_code)]

use std::path::{Path, PathBuf};
use url::Url;

use super::{Mode, TlsModeArg};

// ── Raw config (CLI + v2 TOML → this → ValidatedConfig) ──

/// Flat configuration with all fields resolved.
///
/// Built from CLI flags or v2 TOML, then validated into `ValidatedConfig`.
#[derive(Debug, Clone)]
pub struct RawConfig {
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
    pub pending_db: PathBuf,
    pub max_upload_retries: u32,
    pub dead_letter_dir: PathBuf,
    pub config_path: Option<PathBuf>,
    pub nomad_meta_enabled: bool,
    pub nomad_meta_poll_secs: u64,
}

// ── Defaults ─────────────────────────────────────────────────────

pub fn default_raw() -> RawConfig {
    RawConfig {
        mode: Mode::Standalone,
        service: "default".to_string(),
        watch_path: PathBuf::from("/var/lib/hoard/volumes"),
        watch_patterns: vec!["*".to_string()],
        watch_excludes: vec!["*.tmp".to_string(), "*.journal".to_string()],
        tls_mode: TlsModeArg::Plain,
        s3_endpoint: Url::parse("http://localhost:9000").expect("hardcoded URL"),
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

/// Create a single default volume for no-config-file deployments.
pub fn default_single_volume() -> Vec<super::v2::ResolvedVolume> {
    vec![super::v2::ResolvedVolume {
        name: "default".into(),
        match_glob: "*".into(),
        s3_prefix: "default".into(),
        ttl: "30d".into(),
        retries: 5,
        extensions: vec!["*".into()],
        exclude: vec!["*.tmp".into(), "*.journal".into()],
        compression: None,
        encryption: false,
        on_stop: super::v2::OnStop::Drain,
        on_delete: super::v2::OnDelete::Keep,
        base_dir: None,
    }]
}

// ── v2 → RawConfig mapping ───────────────────────────────────────

/// Map a v2 `ConfigV2` to a flat `RawConfig`, keeping the config path.
pub fn v2_to_raw(v2: &super::v2::ConfigV2, config_path: &Path) -> RawConfig {
    let mut def = default_raw();
    def.config_path = Some(config_path.to_path_buf());

    // [daemon]
    if let Some(ref m) = v2.daemon.mode {
        def.mode = match m.as_str() {
            "nomad" => Mode::Nomad,
            _ => Mode::Standalone,
        };
    }
    if let Some(ref s) = v2.daemon.service {
        def.service = s.clone();
    }
    if let Some(ref a) = v2.daemon.metrics_addr {
        def.metrics_addr = a.clone();
    }
    if let Some(ref s) = v2.daemon.control_socket {
        def.control_socket = PathBuf::from(s);
    }

    // [watch]
    let watch_paths = &v2.watch.paths;
    if !watch_paths.is_empty() {
        def.watch_path = PathBuf::from(&watch_paths[0]);
    }

    // [s3]
    if !v2.s3.endpoint.is_empty() {
        if let Ok(url) = Url::parse(&v2.s3.endpoint) {
            def.s3_endpoint = url;
        }
    }
    if !v2.s3.region.is_empty() {
        def.s3_region = v2.s3.region.clone();
    }
    if !v2.s3.bucket.is_empty() {
        def.s3_bucket = v2.s3.bucket.clone();
    }
    if !v2.s3.access_key.is_empty() {
        def.s3_access_key = v2.s3.access_key.clone();
    }
    if !v2.s3.secret_key.is_empty() {
        def.s3_secret_key = v2.s3.secret_key.clone();
    }
    def.s3_no_sign = v2.s3.no_sign;

    // [nomad]
    if let Some(ref a) = v2.nomad.addr {
        def.nomad_addr = Some(a.clone());
    }
    if let Some(ref t) = v2.nomad.token {
        def.nomad_token = Some(t.clone());
    }
    def.nomad_meta_enabled = v2.nomad.meta_enabled;
    def.nomad_meta_poll_secs = v2.nomad.meta_poll_secs.unwrap_or(300);

    // [defaults] (applied to volumes in v2.rs resolve step)
    if let Some(ref pats) = v2.defaults.extensions {
        def.watch_patterns = pats.iter().map(|e| format!("*.{e}")).collect();
    }
    if let Some(ref ex) = v2.defaults.exclude {
        def.watch_excludes = ex.clone();
    }
    if let Some(ref prefix) = v2.defaults.prefix {
        def.s3_prefix = prefix.clone();
    }

    // [resilience]
    if let Some(ref db) = v2.resilience.pending_db {
        def.pending_db = PathBuf::from(db);
    }
    if let Some(r) = v2.resilience.max_upload_retries {
        def.max_upload_retries = r;
    }
    if let Some(ref d) = v2.resilience.dead_letter_dir {
        def.dead_letter_dir = PathBuf::from(d);
    }

    def
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_defaults_are_standalone() {
        let raw = default_raw();
        assert_eq!(raw.mode, Mode::Standalone);
        assert_eq!(raw.max_upload_retries, 5);
        assert_eq!(raw.gc_interval_secs, 3600);
    }

    #[test]
    fn default_single_volume_has_expected_prefix() {
        let volumes = default_single_volume();
        assert_eq!(volumes.len(), 1);
        assert_eq!(volumes[0].name, "default");
        assert_eq!(volumes[0].s3_prefix, "default");
    }
}
