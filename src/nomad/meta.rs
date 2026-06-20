//! Nomad meta auto-discovery.
//!
//! Periodically polls the Nomad API to discover jobs with
//! `hoard_enabled = "true"` in their meta, and generates
//! dynamic volume configurations for allocs running locally.
//!
//! ## Meta keys (underscore format for HCL compatibility)
//!
//! | Key | Required | Default | Description |
//! |-----|----------|---------|-------------|
//! | `hoard_enabled` | yes | — | `"true"` to enable hoard for this job |
//! | `hoard_match` | no | `"**/*.db"` | Glob relative to alloc dir |
//! | `hoard_class` | no | — | StorageClass name |
//! | `hoard_prefix` | no | `"backup/{job}/"` | S3 key prefix |
//! | `hoard_ttl` | no | — | TTL override (e.g. "30d") |
//! | `hoard_extensions` | no | — | Comma-separated filter (e.g. "db,wal,shm") |

#![deny(unsafe_code)]

use crate::config::v2::ResolvedVolume;
use crate::nomad::client::NomadClient;

/// Meta auto-discovery engine.
///
/// Polls the Nomad API every poll interval to discover running jobs
/// with `hoard_enabled = "true"` meta, and returns resolved volumes
/// for any job with an alloc on the current node.
pub struct MetaDiscovery {
    client: NomadClient,
}

impl MetaDiscovery {
    /// Create a new meta discovery engine.
    pub fn new(
        nomad_addr: &str,
        nomad_token: Option<&str>,
        _watch_root: &str,
        _poll_secs: u64,
    ) -> Self {
        Self {
            client: NomadClient::new(
                nomad_addr.to_string(),
                nomad_token.unwrap_or("").to_string(),
            ),
        }
    }

    /// Discover hoard-enabled volumes from Nomad.
    ///
    /// Returns `ResolvedVolume` entries (sorted, ready to reload into
    /// the volume registry).
    pub async fn discover(&self) -> Result<Vec<ResolvedVolume>, anyhow::Error> {
        let volumes = self.client.discover_volumes().await?;

        let mut resolved = Vec::with_capacity(volumes.len());
        for dv in volumes {
            let s3_prefix = dv
                .s3_prefix
                .unwrap_or_else(|| format!("backup/{}", dv.job_id));
            let ttl = dv.ttl.unwrap_or_else(|| "30d".to_string());

            let base_dir = dv.alloc_dir.map(std::path::PathBuf::from);

            resolved.push(ResolvedVolume {
                name: format!("nomad-{}", dv.job_id),
                match_glob: dv.match_glob,
                s3_prefix,
                ttl,
                retries: 5,
                extensions: dv.extensions.unwrap_or_default(),
                exclude: Vec::new(),
                compression: None,
                encryption: false,
                on_stop: crate::config::v2::OnStop::Drain,
                on_delete: crate::config::v2::OnDelete::Keep,
                base_dir,
            });
        }

        Ok(resolved)
    }
}
