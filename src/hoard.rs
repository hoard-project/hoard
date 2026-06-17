//! Hoard lifecycle state machine.
//!
//! ```text
//! HoardStopped → load_ebpf() → EbpfAttached
//! EbpfAttached    → activate()   → HoardReady
//! HoardReady   → run()        → shuts down gracefully
//! ```
//!
//! The main loop routes events from three sources:
//! - BPF RingBuffer (file writes) → inode resolution → debounce → upload
//! - Trigger (IPC flush / Nomad drain) → upload all pending files
//! - GC timer → periodic S3 cleanup

#![deny(unsafe_code)]

use crate::config::{Mode, ValidatedConfig};
use crate::ebpf::resolve::InodeCache;
use crate::ebpf::{BpfProgram, FileFilter};
use crate::s3::{S3Backend, VerifiedS3Backend};
use crate::trigger::TriggerSource;
use anyhow::{Context, Result};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

// ── State: Stopped ───────────────────────────────────────────────

pub struct HoardStopped {
    config: ValidatedConfig,
}

impl HoardStopped {
    pub fn new(config: ValidatedConfig) -> Self {
        Self { config }
    }

    pub async fn load_ebpf(self) -> Result<EbpfAttached> {
        let bp = BpfProgram::load().await?;
        tracing::info!("eBPF programs loaded");
        Ok(EbpfAttached {
            ebpf: bp,
            config: self.config,
        })
    }
}

// ── State: eBPF attached ─────────────────────────────────────────

pub struct EbpfAttached {
    ebpf: BpfProgram,
    config: ValidatedConfig,
}

impl EbpfAttached {
    async fn verify_s3(&self) -> Result<VerifiedS3Backend> {
        S3Backend::new(
            self.config.s3_access_key.clone(),
            self.config.s3_secret_key.clone(),
            self.config.s3_region.clone(),
            self.config.s3_endpoint.to_string(),
            self.config.s3_bucket.clone(),
            self.config.s3_no_sign,
        )
        .verify()
        .await
    }

    async fn build_trigger(&self) -> Result<TriggerSource> {
        match self.config.mode {
            Mode::Standalone => {
                let (flush_tx, flush_rx) = tokio::sync::mpsc::channel(16);
                let term = crate::trigger::standalone::sigterm_signal().await?;
                let sock =
                    crate::trigger::standalone::bind_control_socket(&self.config.control_socket)
                        .await?;
                let flush_tx2 = flush_tx.clone();
                tokio::spawn(async move {
                    loop {
                        if let Ok(Some(stream)) =
                            crate::trigger::standalone::accept_control(&sock).await
                        {
                            let _ = flush_tx2.send(()).await;
                            drop(stream);
                        }
                    }
                });
                Ok(TriggerSource::Standalone { flush_rx, term })
            }
            Mode::Nomad => {
                let addr = self
                    .config
                    .nomad_addr
                    .as_deref()
                    .context("--nomad-addr required for Nomad mode")?;
                let token = self.config.nomad_token.clone();
                let sse = crate::trigger::nomad::NomadEventStream::connect(addr, token).await?;
                Ok(TriggerSource::Nomad { sse })
            }
        }
    }

    pub async fn activate(self) -> Result<HoardReady> {
        let s3 = self.verify_s3().await?;
        let trigger = self.build_trigger().await?;
        let filter = FileFilter::new(
            self.config.watch_path.clone(),
            &self.config.watch_patterns,
            &self.config.watch_excludes,
        )
        .context("invalid watch pattern")?;
        tracing::info!(mode = ?self.config.mode, "Hoard ready");
        Ok(HoardReady {
            ebpf: self.ebpf,
            s3,
            trigger,
            config: self.config,
            filter,
            inode_cache: Arc::new(InodeCache::new()),
        })
    }
}

// ── State: Ready (main loop) ─────────────────────────────────────

/// Reloadable GC state, updated on SIGHUP.
struct GcReloadState {
    ttl: Duration,
    prefix: String,
}

pub struct HoardReady {
    ebpf: BpfProgram,
    s3: VerifiedS3Backend,
    trigger: TriggerSource,
    config: ValidatedConfig,
    filter: FileFilter,
    inode_cache: Arc<InodeCache>,
}

impl HoardReady {
    pub async fn run(self) -> Result<()> {
        let gc_interval = Duration::from_secs(self.config.gc_interval_secs);
        let watch_root = Arc::new(self.config.watch_path.clone());
        let s3_prefix = Arc::new(self.config.s3_prefix.clone());
        let filter = Arc::new(Mutex::new(self.filter));
        let gc_state = Arc::new(Mutex::new(GcReloadState {
            ttl: Duration::from_secs(u64::from(self.config.gc_ttl_days) * 86400),
            prefix: match self.config.mode {
                Mode::Standalone => self.config.service.clone(),
                Mode::Nomad => String::from("nomad"),
            },
        }));

        let mut gc_timer = tokio::time::interval(gc_interval);
        gc_timer.tick().await;

        let ebpf = Arc::new(Mutex::new(self.ebpf));
        let s3 = Arc::new(self.s3);
        let config = Arc::new(self.config);

        // Flush channel — triggered by metrics /flush endpoint
        #[allow(unused_variables)]
        let (flush_tx, mut flush_rx) = tokio::sync::mpsc::unbounded_channel::<()>();

        // SIGTERM handler — graceful drain on shutdown
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .context("failed to register SIGTERM handler")?;
        // Also catch SIGINT (Ctrl+C) for manual testing
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .context("failed to register SIGINT handler")?;
        // SIGHUP for config reload
        let mut sighup = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
            .context("failed to register SIGHUP handler")?;

        // ── Start Prometheus metrics server ──
        #[cfg(feature = "prometheus")]
        {
            let metrics_addr = config.metrics_addr.clone();
            let tx = flush_tx.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::metrics::serve_metrics(&metrics_addr, Some(tx)).await {
                    tracing::error!(%e, "metrics server failed");
                }
            });
        }

        // Nomad mode: periodic drain every 10 minutes (in addition to SSE triggers)
        let drain_interval = if config.mode == Mode::Nomad {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(u64::MAX) // effectively never
        };
        let mut periodic_drain = tokio::time::interval(drain_interval);
        periodic_drain.tick().await; // skip first immediate tick
        let mut trigger_events = self.trigger.into_channel();

        // BPF events channel: (dev, ino) pairs from the kernel
        let (bpf_tx, mut bpf_rx) = tokio::sync::mpsc::unbounded_channel();

        // Spawn BPF polling task
        {
            let ebpf = ebpf.clone();
            let tx = bpf_tx.clone();
            tokio::spawn(async move {
                let mut ctr: u64 = 0;
                loop {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    if let Ok(Some(e)) = ebpf.lock().await.poll(Duration::from_millis(0)) {
                        ctr += 1;
                        // Log every 100th event
                        if ctr % 100 == 1 {
                            tracing::debug!(
                                count = ctr,
                                dev = e.dev,
                                ino = e.ino,
                                "BPF event received"
                            );
                        }
                        let _ = tx.send(e);
                        #[cfg(feature = "prometheus")]
                        crate::metrics::RINGBUF_EVENTS_TOTAL.inc();
                    }
                }
            });
        }

        // Set of files pending upload (canonical paths)
        let pending: Arc<Mutex<HashSet<std::path::PathBuf>>> = Arc::new(Mutex::new(HashSet::new()));
        let inode_cache = self.inode_cache.clone();

        tracing::info!(mode = ?config.mode, watch_root = %watch_root.display(), "entering main event loop");

        loop {
            tokio::select! {
                // ── BPF event → resolve → debounce → queue ──
                Some(ev) = bpf_rx.recv() => {
                    let watch = watch_root.clone();
                    let pend = pending.clone();
                    let cache = inode_cache.clone();

                    // Spawn as fire-and-forget — concurrent debounce for high throughput
                    let filt = filter.clone();
                    tokio::spawn(async move {
                        let path = {
                            let f = filt.lock().await;
                            // Resolve inode once, check filter, drop lock before debounce
                            match cache.resolve(&watch, ev.dev, ev.ino).await {
                                Some(p) if f.should_monitor(&p) => Some(p),
                                _ => None,
                            }
                        }; // filter lock dropped here
                        if let Some(path) = path {
                            Self::debounce_and_queue(&path, &pend).await;
                        }
                    });
                }

                // ── Trigger (flush/drain) → upload all pending ──
                trigger = trigger_events.recv() => {
                    match trigger {
                        Some(t) => {
                            tracing::info!(trigger_type = t, "trigger fired, draining pending files");
                            let to_upload: Vec<_> = {
                                let mut guard = pending.lock().await;
                                let files: Vec<_> = guard.drain().collect();
                                files
                            };
                            for path in &to_upload {
                                Self::upload_file(&s3, path, &s3_prefix).await;
                            }
                            tracing::info!(count = to_upload.len(), "drain complete");
                        }
                        None => {
                            tracing::info!("trigger channel closed, shutting down");
                            break;
                        }
                    }
                }

                // ── HTTP flush trigger ──
                _ = flush_rx.recv() => {
                    tracing::info!("HTTP flush triggered, draining pending files");
                    let to_upload: Vec<_> = {
                        let mut guard = pending.lock().await;
                        let files: Vec<_> = guard.drain().collect();
                        files
                    };
                    for path in &to_upload {
                        Self::upload_file(&s3, path, &s3_prefix).await;
                    }
                    tracing::info!(count = to_upload.len(), "flush drain complete");
                }

                // ── GC timer ──
                _ = gc_timer.tick() => {
                    let (prefix, ttl) = {
                        let gs = gc_state.lock().await;
                        (gs.prefix.clone(), gs.ttl)
                    }; // lock dropped before await — prevents SIGHUP deadlock
                    tracing::info!("GC timer fired");
                    match crate::s3::gc::gc_cycle(&s3, &prefix, ttl).await {
                        Ok(stats) => {
                            tracing::info!(?stats, "GC cycle complete");
                            #[cfg(feature = "prometheus")]
                            {
                                crate::metrics::GC_CYCLES_TOTAL.inc();
                                crate::metrics::GC_DELETED_TOTAL.inc_by(stats.deleted as f64);
                                crate::metrics::GC_ERRORS_TOTAL.inc_by(stats.errors as f64);
                            }
                        }
                        Err(e) => tracing::error!(%e, "GC cycle failed"),
                    }
                }

                // ── Periodic drain (Nomad mode: every 10 min) ──
                _ = periodic_drain.tick() => {
                    tracing::info!("periodic drain timer fired");
                    let to_upload: Vec<_> = {
                        let mut guard = pending.lock().await;
                        let files: Vec<_> = guard.drain().collect();
                        files
                    };
                    for path in &to_upload {
                        Self::upload_file(&s3, path, &s3_prefix).await;
                    }
                    if !to_upload.is_empty() {
                        tracing::info!(count = to_upload.len(), "periodic drain complete");
                    }
                }

                // ── SIGTERM / SIGINT → graceful drain and exit ──
                _ = sigterm.recv() => {
                    tracing::warn!("SIGTERM received, draining pending files before exit");
                    let to_upload: Vec<_> = {
                        let mut guard = pending.lock().await;
                        let files: Vec<_> = guard.drain().collect();
                        files
                    };
                    for path in &to_upload {
                        Self::upload_file(&s3, path, &s3_prefix).await;
                    }
                    tracing::warn!(count = to_upload.len(), "SIGTERM drain complete, exiting");
                    break;
                }

                _ = sigint.recv() => {
                    tracing::warn!("SIGINT received, draining pending files before exit");
                    let to_upload: Vec<_> = {
                        let mut guard = pending.lock().await;
                        let files: Vec<_> = guard.drain().collect();
                        files
                    };
                    for path in &to_upload {
                        Self::upload_file(&s3, path, &s3_prefix).await;
                    }
                    tracing::warn!(count = to_upload.len(), "SIGINT drain complete, exiting");
                    break;
                }

                // ── SIGHUP → reload config (filter + GC) ──
                _ = sighup.recv() => {
                    tracing::info!("SIGHUP received, reloading configuration");
                    match reload_config(&config, &filter, &gc_state, &mut gc_timer).await {
                        Ok(()) => tracing::info!("config reloaded successfully"),
                        Err(e) => tracing::error!(%e, "config reload failed"),
                    }
                }
            }
        }

        tracing::info!("Hoard shutting down");
        Ok(())
    }
}

/// Reload filter + GC settings from the TOML config file on SIGHUP.
/// GC interval changes don't affect the active timer (restart needed for that).
async fn reload_config(
    config: &Arc<ValidatedConfig>,
    filter: &Arc<Mutex<FileFilter>>,
    gc_state: &Arc<Mutex<GcReloadState>>,
    _gc_timer: &mut tokio::time::Interval,
) -> Result<()> {
    let cfg_path = config
        .config_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no config path — SIGHUP reload requires --config"))?;

    // Re-read the TOML file
    let file = crate::config::ConfigFile::load(cfg_path)?;

    // ── Reload file filter ──
    // Only rebuild if TOML has filter config; otherwise skip.
    let has_filter = file.filter.extensions.is_some() || file.filter.exclude.is_some();
    if has_filter {
        let patterns: Vec<String> = file
            .filter
            .extensions
            .map(|exts| exts.iter().map(|e| format!("*.{e}")).collect())
            .unwrap_or_else(|| vec!["*".into()]);
        let excludes: Vec<String> = file.filter.exclude.unwrap_or_default();
        let new_filter = FileFilter::new(config.watch_path.clone(), &patterns, &excludes)
            .context("failed to rebuild filter from reloaded config")?;
        let _old = std::mem::replace(&mut *filter.lock().await, new_filter);
        tracing::info!("filter reloaded from config");
    }

    // ── Reload GC settings ──
    {
        let mut gs = gc_state.lock().await;
        if let Some(ttl_days) = file.gc.ttl_days {
            let new_ttl = Duration::from_secs(u64::from(ttl_days) * 86400);
            tracing::info!(
                old_ttl_secs = gs.ttl.as_secs(),
                new_ttl_secs = new_ttl.as_secs(),
                "GC TTL reloaded"
            );
            gs.ttl = new_ttl;
        }
    }

    Ok(())
}

impl HoardReady {
    /// Debounce → queue for upload. Caller has already resolved path + passed filter.
    async fn debounce_and_queue(
        path: &std::path::Path,
        pending: &Arc<Mutex<HashSet<std::path::PathBuf>>>,
    ) {
        let debouncer = crate::ebpf::debounce::Debouncer::new();
        match debouncer.check_stable(path) {
            Ok(Some(stable)) => {
                tracing::info!(
                    path = %stable.path.display(),
                    size = stable.size,
                    "file stable, queuing for upload"
                );
                // Queue for the next flush/drain trigger
                pending.lock().await.insert(path.to_path_buf());
            }
            Ok(None) => {
                tracing::debug!(path = %path.display(), "file still changing, skipped");
            }
            Err(e) => {
                tracing::error!(path = %path.display(), %e, "debounce failed");
            }
        }
    }

    /// Upload a single file through the full pipeline.
    async fn upload_file(_s3: &VerifiedS3Backend, path: &std::path::Path, _prefix: &str) {
        #[cfg(feature = "prometheus")]
        crate::metrics::UPLOAD_TOTAL.inc();

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.db");

        // Open and stat the file
        let std_file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) => {
                tracing::error!(path = %path.display(), %e, "failed to open file for upload");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                return;
            }
        };
        let file_size = match std_file.metadata() {
            Ok(m) => m.len(),
            Err(e) => {
                tracing::error!(path = %path.display(), %e, "failed to stat file");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                return;
            }
        };

        let file_fd = crate::fd::FileFd::from_file(std_file);
        let s3_key = format!("{file_name}");

        // Stage 1: WAL checkpoint (sync, blocks briefly)
        let checkpointed = match crate::upload::pipeline::UploadPipeline::new(
            file_fd,
            file_size,
            s3_key.clone(),
            path.to_path_buf(),
        )
        .wal_checkpoint()
        {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(s3_key, %e, "WAL checkpoint failed");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                return;
            }
        };

        // Stage 2: Presign (async)
        let presigned = match checkpointed.presign(_s3).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(s3_key, %e, "S3 presign failed");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                return;
            }
        };

        // Stage 3: Connect (type-state pass-through; actual connection in write_header)
        let connected = match presigned.connect("localhost", 9000).await {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(s3_key, %e, "TCP connect failed");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                return;
            }
        };

        // Stage 4: Write header + sendfile body (sync, use kTLS if available)
        let (header_written, sock) = match connected.write_header(None) {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!(s3_key, %e, "header write failed");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                return;
            }
        };

        let body_sent = match header_written.sendfile_body(&sock) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!(s3_key, %e, "sendfile failed");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                return;
            }
        };

        // Stage 5: Shutdown + read response
        match body_sent.shutdown_and_read(sock) {
            Ok(outcome) if outcome.is_success() => {
                tracing::info!(s3_key, status = outcome.status_code, etag = ?outcome.etag(), "upload succeeded");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_BYTES_TOTAL.inc_by(file_size as f64);
            }
            Ok(outcome) => {
                tracing::error!(s3_key, status = outcome.status_code, error = ?outcome.error(), "upload failed");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
            }
            Err(e) => {
                tracing::error!(s3_key, %e, "shutdown/read failed");
                #[cfg(feature = "prometheus")]
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
            }
        }
    }
}
