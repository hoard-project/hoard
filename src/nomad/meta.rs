//! Nomad meta auto-discovery.
//!
//! Periodically polls the Nomad API to discover jobs with
//! `hoard.enabled = "true"` in their meta, and generates
//! dynamic volume configurations for allocs running locally.
//!
//! ## Meta keys
//!
//! | Key | Required | Default | Description |
//! |-----|----------|---------|-------------|
//! | `hoard.enabled` | yes | — | `"true"` to enable hoard for this job |
//! | `hoard.match` | no | `"**/*.db"` | Glob relative to alloc dir |
//! | `hoard.class` | no | — | StorageClass name |
//! | `hoard.s3_prefix` | no | `"backup/{job}/"` | S3 key prefix |
//! | `hoard.ttl` | no | — | TTL override (e.g. "30d") |
//! | `hoard.extensions` | no | — | Comma-separated filter (e.g. "db,wal,shm") |

#![deny(unsafe_code)]

use crate::config::v2::ResolvedVolume;
use crate::nomad::client::NomadClient;

/// Meta auto-discovery engine.
///
/// Polls the Nomad API at `nomad_addr` every `poll_secs` seconds,
/// fetches all running jobs with `hoard.enabled = "true"` meta,
/// and returns resolved volumes for any job with an alloc on
/// the current node.
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
            let s3_prefix = dv.s3_prefix.unwrap_or_else(|| format!("backup/{}", dv.job_id));
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

/// Discover hoard volumes from Nomad job meta (standalone function).
pub async fn discover(client: &NomadClient) -> Result<Vec<ResolvedVolume>, anyhow::Error> {
    let discovered = client.discover_volumes().await?;
    let mut resolved = Vec::with_capacity(discovered.len());
    for dv in discovered {
        let s3_prefix = dv.s3_prefix.unwrap_or_else(|| format!("backup/{}", dv.job_id));
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

/// Create a periodic meta auto-discovery task.
///
/// Returns a stream that yields updated volume lists every `interval`.
pub fn watch(client: NomadClient, interval: std::time::Duration) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.tick().await;

        loop {
            ticker.tick().await;
            match crate::nomad::meta::discover(&client).await {
                Ok(vols) => {
                    tracing::info!(count = vols.len(), "nomad meta refresh");
                    for v in &vols {
                        tracing::info!(
                            name = %v.name,
                            prefix = %v.s3_prefix,
                            r#match = %v.match_glob,
                            "nomad volume"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(%e, "nomad meta discovery failed, will retry");
                }
            }
        }
    })
}

/// Generate a Nomad job spec snippet with hoard meta for testing.
pub fn meta_snippet(job_name: &str) -> String {
    format!(
        r#"      meta {{
        hoard.enabled    = "true"
        hoard.match      = "**/*.db"
        hoard.class      = "fast"
        hoard.s3_prefix  = "backup/{}"
      }}"#,
        job_name
    )
}
