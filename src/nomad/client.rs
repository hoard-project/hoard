//! Nomad HTTP API client for job meta discovery.
//!
//! Queries the Nomad agent API to discover jobs with `hoard.*` meta
//! keys and generate virtual volume configurations.

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::Duration;

// ── Nomad API models ──────────────────────────────────────────────

/// Job stub from `GET /v1/jobs`
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct JobStub {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Status")]
    #[serde(default)]
    pub status: String,
    #[serde(rename = "Type")]
    #[serde(default)]
    pub job_type: String,
}

/// Full job from `GET /v1/job/{id}`
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Job {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "Status")]
    #[serde(default)]
    pub status: String,
    #[serde(rename = "Meta")]
    #[serde(default)]
    pub meta: Option<HashMap<String, String>>,
    #[serde(rename = "TaskGroups")]
    #[serde(default)]
    pub task_groups: Vec<TaskGroup>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct TaskGroup {
    #[serde(rename = "Tasks")]
    #[serde(default)]
    pub tasks: Vec<Task>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Task {
    #[serde(rename = "Name")]
    pub name: String,
}

/// A discovered volume from Nomad job meta.
#[derive(Debug, Clone)]
pub struct DiscoveredVolume {
    pub job_id: String,
    /// Relative path pattern from alloc dir (e.g. "data/*.db")
    pub match_glob: String,
    /// Storage class name to inherit
    pub class: Option<String>,
    /// Override S3 prefix
    pub s3_prefix: Option<String>,
    /// TTL override
    pub ttl: Option<String>,
    /// Extensions filter
    pub extensions: Option<Vec<String>>,
    /// Absolute path to the Nomad alloc directory
    pub alloc_dir: Option<String>,
}

// ── Client ────────────────────────────────────────────────────────

pub struct NomadClient {
    client: Client,
    addr: String,
    token: String,
}

impl NomadClient {
    pub fn new(addr: String, token: String) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(15))
                .connect_timeout(Duration::from_secs(5))
                .build()
                .expect("reqwest Client"),
            addr,
            token,
        }
    }

    /// List all running jobs.
    pub async fn list_jobs(&self) -> Result<Vec<JobStub>> {
        let url = format!("{}/v1/jobs", self.addr);
        let resp = self
            .client
            .get(&url)
            .header("X-Nomad-Token", &self.token)
            .send()
            .await
            .context("GET /v1/jobs")?;

        let jobs: Vec<JobStub> = resp.json().await.context("parse job list")?;
        Ok(jobs)
    }

    /// Fetch full job detail including meta.
    pub async fn get_job(&self, id: &str) -> Result<Job> {
        let url = format!("{}/v1/job/{}", self.addr, id);
        let resp = self
            .client
            .get(&url)
            .header("X-Nomad-Token", &self.token)
            .send()
            .await
            .with_context(|| format!("GET /v1/job/{}", id))?;

        let job: Job = resp
            .json()
            .await
            .with_context(|| format!("parse job {}", id))?;
        Ok(job)
    }

    /// Fetch job allocations.
    pub async fn get_allocations(&self, job_id: &str) -> Result<Vec<AllocStub>> {
        let url = format!("{}/v1/job/{}/allocations", self.addr, job_id);
        let resp = self
            .client
            .get(&url)
            .header("X-Nomad-Token", &self.token)
            .send()
            .await
            .with_context(|| format!("GET /v1/job/{}/allocations", job_id))?;

        let allocs: Vec<AllocStub> = resp.json().await.context("parse allocations")?;
        Ok(allocs)
    }

    /// Get self node ID.
    pub async fn self_node_id(&self) -> Result<String> {
        let url = format!("{}/v1/agent/self", self.addr);
        let resp = self
            .client
            .get(&url)
            .header("X-Nomad-Token", &self.token)
            .send()
            .await
            .context("GET /v1/agent/self")?;

        let agent: serde_json::Value = resp.json().await.context("parse agent self")?;
        let node_id = agent
            .pointer("/stats/client/node_id")
            .and_then(|v| v.as_str())
            .map(String::from)
            .context("node_id not found in agent self response")?;

        Ok(node_id)
    }

    /// Discover hoard volumes from Nomad job meta.
    ///
    /// Only jobs with `hoard.enabled = "true"` in their meta are included.
    pub async fn discover_volumes(&self) -> Result<Vec<DiscoveredVolume>> {
        let self_node = self.self_node_id().await?;
        let jobs = self.list_jobs().await?;
        let mut volumes = Vec::new();

        for stub in jobs {
            if stub.status != "running" {
                continue;
            }
            let job = match self.get_job(&stub.id).await {
                Ok(j) => j,
                Err(e) => {
                    tracing::warn!(job=%stub.id, %e, "failed to fetch job detail");
                    continue;
                }
            };

            let meta = match &job.meta {
                Some(m) => m,
                None => continue,
            };

            // Only process jobs with hoard_enabled = "true"
            if meta.get("hoard_enabled").map(|v| v.as_str()) != Some("true") {
                continue;
            }

            // Collect meta keys for this job
            let class = meta.get("hoard_class").cloned();
            let s3_prefix = meta.get("hoard_prefix").cloned();
            let ttl = meta.get("hoard_ttl").cloned();
            let match_glob = meta
                .get("hoard_match")
                .cloned()
                .unwrap_or_else(|| "**/*.db".to_string());
            let extensions = meta.get("hoard_extensions").map(|exts| {
                exts.split(',')
                    .map(|s| s.trim().to_string())
                    .collect::<Vec<_>>()
            });

            // Check: is any alloc for this job running on our node?
            let is_local = self
                .get_allocations(&stub.id)
                .await
                .map(|allocs| {
                    allocs
                        .iter()
                        .any(|a| a.node_id == self_node && a.client_status == "running")
                })
                .unwrap_or(false);

            if is_local {
                // Find the running alloc on this node to get its directory
                let alloc_dir = self
                    .get_allocations(&stub.id)
                    .await
                    .ok()
                    .and_then(|allocs| {
                        allocs
                            .iter()
                            .find(|a| a.node_id == self_node && a.client_status == "running")
                            .map(|a| {
                                // Nomad convention: {data_dir}/alloc/{alloc_id}/alloc/
                                format!("/opt/nomad/data/alloc/{}/alloc", a.id)
                            })
                    });

                let vol = DiscoveredVolume {
                    job_id: stub.id.clone(),
                    match_glob,
                    class,
                    s3_prefix,
                    ttl,
                    extensions,
                    alloc_dir,
                };
                tracing::info!(
                    job=%vol.job_id,
                    match_glob=%vol.match_glob,
                    class=?vol.class,
                    "discovered Nomad job volume"
                );
                volumes.push(vol);
            }
        }

        Ok(volumes)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AllocStub {
    #[serde(rename = "ID")]
    pub id: String,
    #[serde(rename = "NodeID")]
    pub node_id: String,
    #[serde(rename = "ClientStatus")]
    pub client_status: String,
}
