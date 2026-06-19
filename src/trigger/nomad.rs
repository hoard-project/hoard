#![allow(dead_code)]
//! Nomad mode trigger: event stream-based allocation monitoring.
//!
//! Connects to the Nomad agent's event stream (`/v1/event/stream`)
//! to detect allocation lifecycle changes (Drain, Stop, Fail).
//!
//! ## Architecture
//!
//! ```text
//! Nomad agent ──── SSE/JSON Lines ──── NomadEventStream
//!                                        │
//!                                        ├── AllocationUpdated (Complete/Failed/Lost)
//!                                        └── drain:job:alloc_id → main loop → upload
//! ```

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use bytes::Bytes;
use futures_util::StreamExt;
use serde::Deserialize;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};

// ── Event types ──────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AllocationEvent {
    pub job: String,
    pub alloc_id: String,
    pub status: AllocStatus,
    pub node_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllocStatus {
    Running,
    Pending,
    Complete,
    Failed,
    Lost,
}

impl AllocStatus {
    fn from_client_status(s: &str) -> Self {
        match s {
            "running" => Self::Running,
            "pending" => Self::Pending,
            "complete" => Self::Complete,
            "failed" => Self::Failed,
            "lost" => Self::Lost,
            _ => Self::Running,
        }
    }
}

impl AllocationEvent {
    pub fn is_drain(&self) -> bool {
        matches!(
            self.status,
            AllocStatus::Complete | AllocStatus::Failed | AllocStatus::Lost
        )
    }

    pub fn s3_key(&self) -> String {
        format!("backup/{}/alloc_{}/", self.job, self.alloc_id)
    }

    pub fn local_path(&self, root: &std::path::Path) -> PathBuf {
        root.join(&self.job).join(&self.alloc_id)
    }
}

// ── JSON models (Nomad v2 event stream format) ───────────────────

#[derive(Debug, Deserialize)]
struct EventFrame {
    #[serde(default, rename = "Events")]
    events: Vec<EventEntry>,
}

#[derive(Debug, Deserialize)]
struct EventEntry {
    #[serde(default, rename = "Topic")]
    topic: String,
    #[serde(default, rename = "Type")]
    event_type: String,
    #[serde(default, rename = "Payload")]
    payload: EventPayload,
}

#[derive(Debug, Default, Deserialize)]
struct EventPayload {
    #[serde(default, rename = "Allocation")]
    allocation: Option<AllocationBody>,
}

#[derive(Debug, Deserialize)]
struct AllocationBody {
    #[serde(rename = "ID", default)]
    id: String,
    #[serde(rename = "JobID", default)]
    job_id: String,
    #[serde(rename = "ClientStatus", default)]
    client_status: String,
    #[serde(rename = "NodeID", default)]
    node_id: String,
}

// ── Stream ───────────────────────────────────────────────────────

/// Persistent Nomad event stream using JSON Lines.
///
/// Maintains a single long-lived HTTP connection. Each call to
/// [`next`](Self::next) reads the next allocation event from the
/// stream. On disconnect, automatically reconnects.
pub struct NomadEventStream {
    base_url: String,
    acl_token: Option<String>,
    client: reqwest::Client,
    namespace: String,
    /// Persistent line reader (None = need to connect)
    reader: Option<ReaderState>,
}

/// Internal state wrapping the streaming response and line reader.
struct ReaderState {
    /// Boxed async-reader wrapping the response's byte stream.
    /// This is type-erased because `reqwest::bytes_stream()` returns
    /// an opaque `impl Stream`.
    reader: Pin<Box<dyn tokio::io::AsyncBufRead + Send>>,
}

impl NomadEventStream {
    pub async fn connect(addr: &str, acl_token: Option<String>) -> Result<Self> {
        let client = build_reqwest_client(addr)?;
        let base_url = normalize_addr(addr);

        // Verify connectivity
        Self::verify_connection(&client, &base_url, &acl_token).await?;

        tracing::info!(addr, "connected to Nomad agent");

        Ok(Self {
            base_url,
            acl_token,
            client,
            namespace: "*".to_string(),
            reader: None,
        })
    }

    async fn verify_connection(
        client: &reqwest::Client,
        base_url: &str,
        acl_token: &Option<String>,
    ) -> Result<()> {
        let mut req = client.get(format!("{base_url}/v1/agent/self"));
        if let Some(ref token) = acl_token {
            req = req.header("X-Nomad-Token", token.as_str());
        }
        let resp = req
            .send()
            .await
            .context("failed to connect to Nomad agent")?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Nomad agent returned {status}: {body}");
        }
        Ok(())
    }

    /// Open (or reopen) the SSE connection.
    async fn ensure_connected(&mut self) -> Result<()> {
        if self.reader.is_some() {
            return Ok(());
        }

        let url = format!(
            "{}/v1/event/stream?topic=Allocation&namespace={}",
            self.base_url, self.namespace,
        );

        let mut req = self.client.get(&url);
        if let Some(ref token) = self.acl_token {
            req = req.header("X-Nomad-Token", token.as_str());
        }

        let resp = req.send().await.context("SSE request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("SSE stream returned {status}: {body}");
        }

        // Build a type-erased buffered reader from the byte stream.
        // We pin-box the BufReader because the concrete stream type from
        // reqwest 0.13 is opaque (`impl Stream`), not nameable.
        let stream = resp.bytes_stream().map(
            |r: Result<Bytes, reqwest::Error>| -> std::io::Result<Bytes> {
                r.map_err(std::io::Error::other)
            },
        );
        let stream_reader = tokio_util::io::StreamReader::new(stream);
        let buf_reader: Pin<Box<dyn tokio::io::AsyncBufRead + Send>> =
            Box::pin(BufReader::new(stream_reader));

        self.reader = Some(ReaderState { reader: buf_reader });

        Ok(())
    }

    /// Wait for the next allocation event.
    pub async fn next(&mut self) -> Option<AllocationEvent> {
        let mut retries = 0u32;
        const MAX_RETRIES: u32 = 5;
        const RETRY_DELAY: Duration = Duration::from_secs(2);

        loop {
            // Ensure we have a live SSE connection
            if let Err(e) = self.ensure_connected().await {
                retries += 1;
                if retries > MAX_RETRIES {
                    tracing::error!("SSE connection failed after {MAX_RETRIES} retries: {e:#}");
                    return None;
                }
                tracing::warn!("SSE connection error (retry {retries}/{MAX_RETRIES}): {e}");
                self.reader = None;
                tokio::time::sleep(RETRY_DELAY).await;
                continue;
            }

            // Read one line from the stream
            let reader_state = self
                .reader
                .as_mut()
                .expect("SSE reader should be initialized");
            let mut line = String::new();

            match reader_state.reader.read_line(&mut line).await {
                Ok(0) => {
                    // EOF — server closed connection
                    tracing::debug!("SSE stream ended, reconnecting");
                    self.reader = None;
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("SSE read error: {e}");
                    self.reader = None;
                    retries += 1;
                    if retries > MAX_RETRIES {
                        return None;
                    }
                    tokio::time::sleep(RETRY_DELAY).await;
                    continue;
                }
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Parse JSON line — Nomad v2 events
            match serde_json::from_str::<EventFrame>(line) {
                Ok(frame) => {
                    for entry in frame.events {
                        if entry.topic == "Allocation"
                            && matches!(
                                entry.event_type.as_str(),
                                "AllocationUpdated" | "PlanResult"
                            )
                        {
                            if let Some(alloc) = &entry.payload.allocation {
                                if alloc.id.is_empty() || alloc.job_id.is_empty() {
                                    continue;
                                }

                                let event = AllocationEvent {
                                    job: alloc.job_id.clone(),
                                    alloc_id: alloc.id.clone(),
                                    status: AllocStatus::from_client_status(&alloc.client_status),
                                    node_id: alloc.node_id.clone(),
                                };

                                tracing::debug!(
                                    job = %event.job,
                                    alloc = %event.alloc_id,
                                    status = ?event.status,
                                    "Nomad allocation event",
                                );

                                return Some(event);
                            }
                        }
                    }
                }
                Err(e) => {
                    tracing::debug!("skipping unparseable event line: {e} — {:.200}", line);
                }
            }
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────

fn build_reqwest_client(_addr: &str) -> Result<reqwest::Client> {
    // SSE streams require no HTTP-level timeout (they're long-lived).
    // Only set a connect timeout to detect unreachable agents quickly.
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .no_proxy()
        .build()
        .context("Failed to build reqwest client")
}

fn normalize_addr(addr: &str) -> String {
    if addr.starts_with("unix://") {
        "http://127.0.0.1:4646".to_string()
    } else {
        addr.to_string()
    }
}
