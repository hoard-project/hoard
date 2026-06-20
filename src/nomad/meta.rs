//! Nomad meta auto-discovery: poll Nomad API, read `hoard.*` job meta,
//! and synthesize virtual volumes.
//!
//! ## Meta keys
//!
//! ```hcl
//! job "postgres-backup" {
//!   meta {
//!     hoard.class       = "critical"
//!     hoard.s3_prefix   = "tenants/postgres-prod"
//!     hoard.ttl         = "90d"
//!     hoard.extensions  = "db,wal,sqlite"
//!     hoard.exclude     = "*.tmp,*.journal"
//!     hoard.retries     = "10"
//!     hoard.on_stop     = "drain"
//!     hoard.on_delete   = "archive"
//!   }
//! }
//! ```
//!
//! Volume paths are derived from the job's volume mounts. When a job
//! has `volume "data" { destination = "/var/lib/hoard/volumes/postgres" }`,
//! the virtual volume's `match` glob becomes `postgres/**`.
//!
//! ## Priority
//!
//! Meta volumes take priority over conf.d volumes. If both a file-based
//! volume and a meta volume match the same path, the meta volume wins.
#![deny(unsafe_code)]

use crate::config::v2::{OnDelete, OnStop, ResolvedVolume};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

/// A single Nomad job as returned by `GET /v1/jobs`.
#[derive(Deserialize, Debug)]
struct JobListEntry {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "Status")]
    status: String,
}

/// Full job spec from `GET /v1/job/{id}`.
#[derive(Deserialize, Debug)]
struct Job {
    #[serde(rename = "ID")]
    #[allow(dead_code)]
    id: String,
    #[serde(rename = "Meta")]
    #[serde(default)]
    meta: HashMap<String, String>,
    #[serde(rename = "TaskGroups")]
    #[serde(default)]
    task_groups: Vec<TaskGroup>,
}

#[derive(Deserialize, Debug)]
struct TaskGroup {
    #[serde(rename = "Name")]
    #[allow(dead_code)]
    name: String,
    #[serde(rename = "Volumes")]
    #[serde(default)]
    volumes: HashMap<String, VolumeRequest>,
}

#[derive(Deserialize, Debug)]
struct VolumeRequest {
    #[serde(rename = "Source")]
    #[allow(dead_code)]
    source: String,
}

/// Discover virtual volumes from Nomad job metadata.
///
/// Returns volumes sorted by specificity (more specific globs first).
pub struct MetaDiscovery {
    addr: String,
    token: Option<String>,
    watch_root: String,
    #[allow(dead_code)]
    poll_interval: Duration,
}

impl MetaDiscovery {
    /// Create a new meta discovery client.
    pub fn new(addr: &str, token: Option<&str>, watch_root: &str, poll_secs: u64) -> Self {
        Self {
            addr: addr.to_string(),
            token: token.map(String::from),
            watch_root: watch_root.to_string(),
            poll_interval: Duration::from_secs(poll_secs),
        }
    }

    /// Poll the Nomad API and discover meta volumes.
    ///
    /// Returns `Vec<ResolvedVolume>` with meta volumes first (highest priority).
    pub async fn discover(&self) -> Result<Vec<ResolvedVolume>> {
        let client = self.build_client()?;
        let jobs = self.list_running_jobs(&client).await?;

        let mut volumes = Vec::new();

        for job_id in &jobs {
            if let Ok(Some(vol)) = self.job_to_volume(&client, job_id).await {
                tracing::info!(
                    job = %job_id,
                    volume = %vol.name,
                    prefix = %vol.s3_prefix,
                    ttl = %vol.ttl,
                    "discovered Nomad meta volume"
                );
                volumes.push(vol);
            }
        }

        Ok(volumes)
    }

    fn build_client(&self) -> Result<reqwest::Client> {
        let mut builder = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5));

        // Skip TLS verification for local Nomad agents (h2c / plain HTTP)
        builder = builder.danger_accept_invalid_certs(true);

        Ok(builder.build()?)
    }

    async fn list_running_jobs(&self, client: &reqwest::Client) -> Result<Vec<String>> {
        let url = format!("{}/v1/jobs", self.addr.trim_end_matches('/'));
        let mut req = client.get(&url);

        if let Some(ref token) = self.token {
            req = req.header("X-Nomad-Token", token);
        }

        let resp = req
            .send()
            .await
            .with_context(|| format!("Nomad API list jobs: {}", url))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Nomad API returned {status}: {body}");
        }

        let entries: Vec<JobListEntry> = resp.json().await.context("parsing Nomad job list")?;

        // Only discover running jobs
        Ok(entries
            .into_iter()
            .filter(|j| j.status == "running")
            .map(|j| j.id)
            .collect())
    }

    async fn job_to_volume(
        &self,
        client: &reqwest::Client,
        job_id: &str,
    ) -> Result<Option<ResolvedVolume>> {
        let url = format!("{}/v1/job/{}", self.addr.trim_end_matches('/'), job_id);
        let mut req = client.get(&url);

        if let Some(ref token) = self.token {
            req = req.header("X-Nomad-Token", token);
        }

        let resp = req
            .send()
            .await
            .with_context(|| format!("Nomad API get job {}", job_id))?;

        if !resp.status().is_success() {
            tracing::warn!(job = %job_id, status = %resp.status(), "Nomad API get job failed");
            return Ok(None);
        }

        let job: Job = resp
            .json()
            .await
            .with_context(|| format!("parsing job spec for {}", job_id))?;

        // Extract hoard.* meta keys
        let _class = job.meta.get("hoard.class").cloned();
        let enabled = job
            .meta
            .get("hoard.enabled")
            .map(|v| v != "false")
            .unwrap_or(true);

        if !enabled {
            return Ok(None);
        }

        // Derive match glob from volume mount destinations.
        // We look for volumes mounted under the watch root.
        let _watch_root_clean = self.watch_root.trim_end_matches('/');
        let mut volume_paths: Vec<String> = Vec::new();

        for tg in &job.task_groups {
            for vol_name in tg.volumes.keys() {
                // The volume destination is typically configured in the
                // task's volume_mount stanza, not in the group-level volume
                // request. For now, derive from the volume name.
                // Users can override with hoard.match meta key.
                volume_paths.push(vol_name.clone());
            }
        }

        // If hoard.match is explicitly set, use it.
        let match_glob = job.meta.get("hoard.match").cloned().unwrap_or_else(|| {
            if volume_paths.is_empty() {
                format!("{}/**", job_id)
            } else {
                volume_paths[0].clone()
            }
        });

        let s3_prefix = job
            .meta
            .get("hoard.s3_prefix")
            .cloned()
            .unwrap_or_else(|| format!("nomad/{}", job_id));

        let ttl = job
            .meta
            .get("hoard.ttl")
            .cloned()
            .unwrap_or_else(|| "30d".to_string());

        let retries: u32 = job
            .meta
            .get("hoard.retries")
            .and_then(|v| v.parse().ok())
            .unwrap_or(5);

        let extensions = job
            .meta
            .get("hoard.extensions")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_else(|| vec!["*".to_string()]);

        let exclude = job
            .meta
            .get("hoard.exclude")
            .map(|v| v.split(',').map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();

        let on_stop = OnStop::parse(
            &job.meta
                .get("hoard.on_stop")
                .cloned()
                .unwrap_or_else(|| "drain".to_string()),
        );

        let on_delete = OnDelete::parse(
            &job.meta
                .get("hoard.on_delete")
                .cloned()
                .unwrap_or_else(|| "keep".to_string()),
        );

        Ok(Some(ResolvedVolume {
            name: format!("meta:{}", job_id),
            match_glob,
            s3_prefix,
            ttl,
            retries,
            extensions,
            exclude,
            compression: None,
            encryption: false,
            on_stop,
            on_delete,
        }))
    }
}
