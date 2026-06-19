//! v2 config schema: StorageClass + Volume model.
//!
//! Inspired by Kubernetes StorageClass / PVC:
//!   StorageClass = reusable policy template (TTL, retries, encryption)
//!   Volume       = tenant binding (path glob → class + S3 prefix)

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::env;

// ── TOML schema ──────────────────────────────────────────────────

/// Top-level v2 TOML config.
#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct ConfigV2 {
    pub hoard: HoardMeta,
    #[serde(default)]
    pub daemon: DaemonSection,
    #[serde(default)]
    pub watch: WatchSection,
    pub s3: S3Section,
    #[serde(default)]
    pub defaults: DefaultsSection,
    #[serde(default)]
    pub storage_classes: Vec<StorageClass>,
    #[serde(default)]
    pub volumes: Vec<Volume>,
    #[serde(default)]
    pub nomad: NomadSection,
    #[serde(default)]
    pub resilience: ResilienceSection,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct HoardMeta {
    pub version: u32, // must be 2
    #[serde(default)]
    pub conf_dirs: Vec<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct DaemonSection {
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub service: Option<String>,
    #[serde(default)]
    pub control_socket: Option<String>,
    #[serde(default)]
    pub metrics_addr: Option<String>,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct WatchSection {
    pub paths: Vec<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct S3Section {
    pub endpoint: String,
    #[serde(default)]
    pub region: String,
    pub bucket: String,
    #[serde(default)]
    pub access_key: String,
    #[serde(default)]
    pub secret_key: String,
    #[serde(default)]
    pub no_sign: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct DefaultsSection {
    #[serde(default)]
    pub prefix: Option<String>,
    #[serde(default = "default_ttl")]
    pub ttl: String,
    #[serde(default = "default_retries")]
    pub retries: u32,
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
    #[serde(default)]
    pub compression: Option<String>,
    #[serde(default)]
    pub encryption: Option<bool>,
    #[serde(default = "default_stop")]
    pub on_stop: String,
    #[serde(default = "default_delete")]
    pub on_delete: String,
}

fn default_ttl() -> String { "30d".to_string() }
fn default_retries() -> u32 { 5 }
fn default_stop() -> String { "drain".to_string() }
fn default_delete() -> String { "keep".to_string() }

impl Default for DefaultsSection {
    fn default() -> Self {
        Self {
            prefix: None,
            ttl: default_ttl(),
            retries: default_retries(),
            extensions: None,
            exclude: None,
            compression: None,
            encryption: None,
            on_stop: default_stop(),
            on_delete: default_delete(),
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct StorageClass {
    pub name: String,
    #[serde(default)]
    pub ttl: Option<String>,
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default)]
    pub compression: Option<String>,
    #[serde(default)]
    pub encryption: Option<bool>,
    #[serde(default)]
    pub on_stop: Option<String>,
    #[serde(default)]
    pub on_delete: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(deny_unknown_fields)]
pub struct Volume {
    pub name: String,
    /// Glob pattern relative to watch.paths root.
    pub r#match: String,
    /// StorageClass name to inherit from.
    #[serde(default)]
    pub class: Option<String>,
    /// Override S3 prefix.
    #[serde(default)]
    pub s3_prefix: Option<String>,
    #[serde(default)]
    pub ttl: Option<String>,
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default)]
    pub extensions: Option<Vec<String>>,
    #[serde(default)]
    pub exclude: Option<Vec<String>>,
    #[serde(default)]
    pub compression: Option<String>,
    #[serde(default)]
    pub encryption: Option<bool>,
    #[serde(default)]
    pub on_stop: Option<String>,
    #[serde(default)]
    pub on_delete: Option<String>,
    /// Disable this volume without removing config.
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool { true }

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct NomadSection {
    #[serde(default)]
    pub addr: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default)]
    pub meta_poll_secs: Option<u64>,
    /// Enable Nomad meta auto-discovery.
    #[serde(default)]
    pub meta_enabled: bool,
}

#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
pub struct ResilienceSection {
    #[serde(default)]
    pub pending_db: Option<String>,
    #[serde(default)]
    pub max_upload_retries: Option<u32>,
    #[serde(default)]
    pub dead_letter_dir: Option<String>,
}

// ── Resolved runtime types ───────────────────────────────────────

/// Fully resolved volume config after class inheritance + defaults.
#[derive(Debug, Clone)]
pub struct ResolvedVolume {
    pub name: String,
    pub match_glob: String,
    pub s3_prefix: String,
    pub ttl: String,
    pub retries: u32,
    /// Allowed file extensions (e.g. ["db", "wal", "sqlite"]).
    /// `["*"]` means all extensions.
    pub extensions: Vec<String>,
    /// Glob patterns to exclude (e.g. ["*.tmp", "*.journal"]).
    pub exclude: Vec<String>,
    pub compression: Option<String>,
    pub encryption: bool,
    pub on_stop: OnStop,
    pub on_delete: OnDelete,
}

impl ResolvedVolume {
    /// Check if a file path should be monitored by this volume.
    ///
    /// Returns `false` if:
    /// - The file extension does not match any of `self.extensions`
    ///   (unless extensions contains `"*"`)
    /// - The file name matches any `self.exclude` glob
    pub fn should_monitor(&self, path: &std::path::Path) -> bool {
        // Check extensions filter
        if !self.extensions.is_empty() && !self.extensions.iter().any(|e| e == "*") {
            let ext = path.extension()
                .and_then(|e| e.to_str())
                .unwrap_or("");
            if !self.extensions.iter().any(|allowed| allowed == ext) {
                tracing::debug!(
                    path = %path.display(),
                    ext = ext,
                    allowed = ?self.extensions,
                    volume = %self.name,
                    "BPF: extension not allowed by volume filter"
                );
                return false;
            }
        }

        // Check exclude patterns
        if !self.exclude.is_empty() {
            let filename = path.file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("");
            for pattern in &self.exclude {
                if simple_glob_match(pattern, filename) {
                    tracing::debug!(
                        path = %path.display(),
                        pattern = pattern,
                        volume = %self.name,
                        "BPF: file excluded by volume pattern"
                    );
                    return false;
                }
            }
        }

        true
    }
}

/// Simple glob match for filenames (single segment, no `/`).
/// `*` matches any sequence of characters.
fn simple_glob_match(pattern: &str, filename: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if !pattern.contains('*') {
        return pattern == filename;
    }
    // Split pattern on '*', match prefix+suffix
    if let Some(pos) = pattern.find('*') {
        let prefix = &pattern[..pos];
        let suffix = &pattern[pos + 1..];
        filename.starts_with(prefix) && filename.ends_with(suffix)
            && filename.len() >= prefix.len() + suffix.len()
    } else {
        pattern == filename
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnStop {
    Drain,
    Keep,
    Purge,
}

impl OnStop {
    pub fn parse(s: &str) -> Self {
        match s {
            "drain" => OnStop::Drain,
            "keep" => OnStop::Keep,
            "purge" => OnStop::Purge,
            _ => OnStop::Drain, // safe default
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnDelete {
    Keep,
    Delete,
    Archive,
}

impl OnDelete {
    pub fn parse(s: &str) -> Self {
        match s {
            "keep" => OnDelete::Keep,
            "delete" => OnDelete::Delete,
            "archive" => OnDelete::Archive,
            _ => OnDelete::Keep,
        }
    }
}

// ── Resolution ───────────────────────────────────────────────────

/// Resolve all volumes: apply StorageClass inheritance → defaults fallback.
pub fn resolve_volumes(v2: &ConfigV2) -> Result<Vec<ResolvedVolume>> {
    let classes: HashMap<&str, &StorageClass> = v2.storage_classes.iter()
        .map(|sc| (sc.name.as_str(), sc))
        .collect();

    let mut resolved = Vec::new();

    for vol in &v2.volumes {
        if !vol.enabled {
            continue;
        }

        let class = vol.class.as_deref().and_then(|c| classes.get(c));

        let ttl = vol.ttl.clone()
            .or_else(|| class.and_then(|c| c.ttl.clone()))
            .unwrap_or_else(|| v2.defaults.ttl.clone());

        let retries = vol.retries
            .or_else(|| class.and_then(|c| c.retries))
            .unwrap_or(v2.defaults.retries);

        let s3_prefix = vol.s3_prefix.clone()
            .unwrap_or_else(|| vol.name.clone());

        let extensions = vol.extensions.clone()
            .or_else(|| v2.defaults.extensions.clone())
            .unwrap_or_else(|| vec!["*".to_string()]);

        let exclude = vol.exclude.clone()
            .or_else(|| v2.defaults.exclude.clone())
            .unwrap_or_default();

        let compression = vol.compression.clone()
            .or_else(|| class.and_then(|c| c.compression.clone()))
            .or_else(|| v2.defaults.compression.clone());

        let encryption = vol.encryption
            .or_else(|| class.and_then(|c| c.encryption))
            .or(v2.defaults.encryption)
            .unwrap_or(false);

        let on_stop = OnStop::parse(
            &vol.on_stop.clone()
                .or_else(|| class.and_then(|c| c.on_stop.clone()))
                .unwrap_or_else(|| v2.defaults.on_stop.clone())
        );

        let on_delete = OnDelete::parse(
            &vol.on_delete.clone()
                .or_else(|| class.and_then(|c| c.on_delete.clone()))
                .unwrap_or_else(|| v2.defaults.on_delete.clone())
        );

        resolved.push(ResolvedVolume {
            name: vol.name.clone(),
            match_glob: vol.r#match.clone(),
            s3_prefix,
            ttl,
            retries,
            extensions,
            exclude,
            compression,
            encryption,
            on_stop,
            on_delete,
        });
    }

    // If no volumes defined, create a catch-all default volume.
    if resolved.is_empty() {
        resolved.push(ResolvedVolume {
            name: "default".to_string(),
            match_glob: "**".to_string(),
            s3_prefix: v2.defaults.prefix.clone().unwrap_or_else(|| "default".to_string()),
            ttl: v2.defaults.ttl.clone(),
            retries: v2.defaults.retries,
            extensions: v2.defaults.extensions.clone().unwrap_or_else(|| vec!["*".to_string()]),
            exclude: v2.defaults.exclude.clone().unwrap_or_default(),
            compression: v2.defaults.compression.clone(),
            encryption: v2.defaults.encryption.unwrap_or(false),
            on_stop: OnStop::parse(&v2.defaults.on_stop),
            on_delete: OnDelete::parse(&v2.defaults.on_delete),
        });
    }

    Ok(resolved)
}

// ── Conf.d loading ───────────────────────────────────────────────

/// Load all .toml files from a conf.d directory and merge into the main config.
///
/// Files are loaded in alphabetical order.  Later files append to
/// `storage_classes` and `volumes` vecs; they do NOT overwrite
/// main config's `[daemon]`, `[s3]`, `[watch]`, etc.
pub fn load_conf_dir(dir: &Path, v2: &mut ConfigV2) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }

    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading conf.d dir: {}", dir.display()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map_or(false, |ext| ext == "toml"))
        .collect();

    entries.sort();

    for path in entries {
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        let expanded = env::expand_env(&raw);
        let partial: ConfFilePartial = toml::from_str(&expanded)
            .with_context(|| format!("parsing {}", path.display()))?;

        v2.storage_classes.extend(partial.storage_classes);
        v2.volumes.extend(partial.volumes);
    }

    Ok(())
}

/// Partial TOML for conf.d files — only [[storage_classes]] and [[volumes]].
#[derive(Deserialize, Debug, Default, Clone)]
#[serde(deny_unknown_fields)]
struct ConfFilePartial {
    #[serde(default)]
    storage_classes: Vec<StorageClass>,
    #[serde(default)]
    volumes: Vec<Volume>,
}

// ── Full loader ──────────────────────────────────────────────────

/// Load the complete v2 config: main file + conf.d directories.
pub fn load(main_path: &Path) -> Result<ConfigV2> {
    let raw = std::fs::read_to_string(main_path)
        .with_context(|| format!("reading {}", main_path.display()))?;
    let expanded = env::expand_env(&raw);

    let mut v2: ConfigV2 = toml::from_str(&expanded)
        .with_context(|| format!("parsing {}", main_path.display()))?;

    if v2.hoard.version != 2 {
        anyhow::bail!("config version must be 2, got {}", v2.hoard.version);
    }

    // Load conf.d directories declared in the main config.
    let conf_dirs = v2.hoard.conf_dirs.clone();
    for dir_spec in &conf_dirs {
        let dir_path = if std::path::Path::new(dir_spec).is_absolute() {
            PathBuf::from(dir_spec)
        } else {
            // Relative to the main config file's directory.
            main_path.parent().unwrap_or(Path::new(".")).join(dir_spec)
        };
        load_conf_dir(&dir_path, &mut v2)?;
    }

    Ok(v2)
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimal_v2_parse() {
        let toml_str = r#"
[hoard]
version = 2

[s3]
endpoint = "http://minio:9000"
bucket = "backups"
"#;
        let v2: ConfigV2 = toml::from_str(toml_str).expect("parse v2 minimal");
        assert_eq!(v2.hoard.version, 2);
        assert_eq!(v2.s3.bucket, "backups");
        // defaults filled
        assert_eq!(v2.defaults.ttl, "30d");
        assert_eq!(v2.defaults.retries, 5);
    }

    #[test]
    fn volume_inherits_class() {
        let toml_str = r#"
[hoard]
version = 2

[s3]
endpoint = "http://minio:9000"
bucket = "backups"

[[storage_classes]]
name = "critical"
ttl = "90d"
retries = 10
encryption = true

[[volumes]]
name = "postgres"
match = "postgres/**"
class = "critical"
s3_prefix = "tenants/pg"
extensions = ["db", "wal"]
"#;
        let v2: ConfigV2 = toml::from_str(toml_str).expect("parse");
        let vols = resolve_volumes(&v2).expect("resolve");
        assert_eq!(vols.len(), 1);
        let v = &vols[0];
        assert_eq!(v.name, "postgres");
        assert_eq!(v.ttl, "90d");        // from class
        assert_eq!(v.retries, 10);        // from class
        assert_eq!(v.encryption, true);   // from class
        assert_eq!(v.extensions, vec!["db", "wal"]); // volume override
        assert_eq!(v.s3_prefix, "tenants/pg");
    }

    #[test]
    fn volume_overrides_class() {
        let toml_str = r#"
[hoard]
version = 2

[s3]
endpoint = "http://minio:9000"
bucket = "backups"

[[storage_classes]]
name = "critical"
ttl = "90d"
retries = 10

[[volumes]]
name = "dev"
match = "dev/**"
class = "critical"
ttl = "7d"
retries = 3
"#;
        let v2: ConfigV2 = toml::from_str(toml_str).expect("parse");
        let vols = resolve_volumes(&v2).expect("resolve");
        let v = &vols[0];
        assert_eq!(v.ttl, "7d");    // volume overrides class
        assert_eq!(v.retries, 3);   // volume overrides class
    }

    #[test]
    fn empty_volumes_gets_default() {
        let toml_str = r#"
[hoard]
version = 2

[s3]
endpoint = "http://minio:9000"
bucket = "backups"

[defaults]
prefix = "catchall"
ttl = "14d"
"#;
        let v2: ConfigV2 = toml::from_str(toml_str).expect("parse");
        let vols = resolve_volumes(&v2).expect("resolve");
        assert_eq!(vols.len(), 1);
        assert_eq!(vols[0].name, "default");
        assert_eq!(vols[0].s3_prefix, "catchall");
        assert_eq!(vols[0].ttl, "14d");
    }

    #[test]
    fn on_stop_on_delete_parsing() {
        assert_eq!(OnStop::parse("drain"), OnStop::Drain);
        assert_eq!(OnStop::parse("keep"), OnStop::Keep);
        assert_eq!(OnStop::parse("purge"), OnStop::Purge);
        assert_eq!(OnStop::parse("invalid"), OnStop::Drain); // fallback

        assert_eq!(OnDelete::parse("keep"), OnDelete::Keep);
        assert_eq!(OnDelete::parse("delete"), OnDelete::Delete);
        assert_eq!(OnDelete::parse("archive"), OnDelete::Archive);
        assert_eq!(OnDelete::parse("invalid"), OnDelete::Keep); // fallback
    }

    #[test]
    fn simple_glob_basics() {
        assert!(simple_glob_match("*", "anything"));
        assert!(simple_glob_match("*.log", "app.log"));
        assert!(!simple_glob_match("*.log", "app.json"));
        assert!(simple_glob_match("app*", "apple"));
        assert!(!simple_glob_match("app*", "xapple"));
        assert!(simple_glob_match("*.tmp", "file.tmp"));
        assert!(!simple_glob_match("*.tmp", "file.txt"));
        assert!(simple_glob_match("data.*", "data.db"));
        assert!(!simple_glob_match("data.*", "my-data.db"));
        assert!(simple_glob_match("file", "file"));
        assert!(!simple_glob_match("file", "filex"));
    }

    #[test]
    fn should_monitor_per_volume() {
        let v = ResolvedVolume {
            name: "test".into(),
            match_glob: "**".into(),
            s3_prefix: "test".into(),
            ttl: "30d".into(),
            retries: 5,
            extensions: vec!["db".into(), "wal".into()],
            exclude: vec!["*.tmp".into()],
            compression: None,
            encryption: false,
            on_stop: OnStop::Drain,
            on_delete: OnDelete::Keep,
        };

        assert!(v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/data.db")));
        assert!(v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/data.wal")));
        assert!(!v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/data.json")));
        assert!(!v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/data.tmp")));
        assert!(!v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/README")));
    }

    #[test]
    fn should_monitor_wildcard_all() {
        let v = ResolvedVolume {
            name: "catch-all".into(),
            match_glob: "**".into(),
            s3_prefix: "catch".into(),
            ttl: "7d".into(),
            retries: 3,
            extensions: vec!["*".into()],
            exclude: vec![],
            compression: None,
            encryption: false,
            on_stop: OnStop::Drain,
            on_delete: OnDelete::Keep,
        };

        assert!(v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/data.db")));
        assert!(v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/data.json")));
        assert!(v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/README")));
        assert!(v.should_monitor(std::path::Path::new("/var/lib/hoard/volumes/test/image.png")));
    }
}
