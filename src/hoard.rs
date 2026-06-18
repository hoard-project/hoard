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
use crate::pending::PersistentPending;
use crate::s3::{S3Backend, VerifiedS3Backend};
use crate::trigger::TriggerSource;
use crate::upload::retry::{write_dead_letter, DeadLetter, RetryConfig};
use anyhow::{Context, Result};
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
                let version = env!("CARGO_PKG_VERSION").to_string();
                let service = self.config.service.clone();
                tokio::spawn(async move {
                    loop {
                        if let Ok(Some(mut stream)) =
                            crate::trigger::standalone::accept_control(&sock).await
                        {
                            let tx = flush_tx2.clone();
                            let ver = version.clone();
                            let svc = service.clone();
                            tokio::spawn(async move {
                                let mut line = String::new();
                                use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
                                let (rx, mut tx_w) = stream.split();
                                let mut reader = BufReader::new(rx);
                                if reader.read_line(&mut line).await.is_ok() {
                                    match crate::trigger::standalone::parse_command(&line) {
                                        Some(crate::trigger::standalone::ControlCommand::Flush) => {
                                            let _ = tx.send(()).await;
                                            let _ = tx_w.write_all(b"ok: flush triggered\n").await;
                                        }
                                        Some(
                                            crate::trigger::standalone::ControlCommand::Status,
                                        ) => {
                                            let status = format!(
                                                "{{\"version\":\"{ver}\",\"mode\":\"standalone\",\"service\":\"{svc}\"}}\n"
                                            );
                                            let _ = tx_w.write_all(status.as_bytes()).await;
                                        }
                                        None => {
                                            let _ =
                                                tx_w.write_all(b"error: unknown command\n").await;
                                        }
                                    }
                                }
                            });
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

        let inode_cache = Arc::new(InodeCache::new());

        // ── Initial scan: baseline upload of all matching files ──
        {
            let scan_result = run_initial_scan(
                &self.config.watch_path,
                &filter,
                &s3,
                &self.config.s3_prefix,
                &inode_cache,
            )
            .await;
            match scan_result {
                Ok(stats) => tracing::info!(?stats, "initial scan complete"),
                Err(e) => tracing::error!(%e, "initial scan failed (non-fatal)"),
            }
        }

        tracing::info!(mode = ?self.config.mode, "Hoard ready");
        Ok(HoardReady {
            ebpf: self.ebpf,
            s3,
            trigger,
            config: self.config,
            filter,
            inode_cache,
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
            prefix: self.config.s3_prefix.clone(),
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
        {
            let metrics_addr = config.metrics_addr.clone();
            tracing::info!(addr = %metrics_addr, "Prometheus metrics endpoint starting");
            let tx = flush_tx.clone();
            tokio::spawn(async move {
                if let Err(e) = crate::metrics::serve_metrics(&metrics_addr, Some(tx)).await {
                    tracing::error!(%e, "metrics server failed");
                }
            });
        }

        // Periodic drain: in Nomad mode every 10 min via SSE triggers,
        // in standalone mode every 30s so BPF-detected changes are
        // actually uploaded without waiting for SIGTERM.
        let drain_interval = if config.mode == Mode::Nomad {
            Duration::from_secs(600)
        } else {
            Duration::from_secs(30) // standalone: drain pending every 30s
        };
        let mut periodic_drain = tokio::time::interval(drain_interval);
        periodic_drain.tick().await; // skip first immediate tick

        // Periodic scan: rediscover files created but never written to
        // (Litestream-style: pick up new databases in subdirectories).
        let mut periodic_scan = tokio::time::interval(Duration::from_secs(1800)); // 30 min
        periodic_scan.tick().await; // skip first — initial scan already did it
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
                        crate::metrics::RINGBUF_EVENTS_TOTAL.inc();
                    }
                }
            });
        }

        // Set of files pending upload (canonical paths)
        // Persistent pending set — survives process restarts
        let pending_db_path = config.pending_db.clone();
        let pending: Arc<Mutex<PersistentPending>> = Arc::new(Mutex::new(
            PersistentPending::open(&pending_db_path)
                .context("failed to open persistent pending database")?,
        ));
        let retry_cfg = RetryConfig {
            max_attempts: config.max_upload_retries,
            ..Default::default()
        };
        let dead_letter_dir = Arc::new(config.dead_letter_dir.clone());
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
                            let to_upload = {
                                let mut guard = pending.lock().await;
                                guard.drain()
                            };
                            for path in &to_upload {
                                if let Err(e) = Self::upload_file(
                                    &s3, path, &watch_root, &s3_prefix,
                                    &retry_cfg, &pending, &dead_letter_dir,
                                ).await {
                                    tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                                }
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
                    let to_upload = {
                        let mut guard = pending.lock().await;
                        guard.drain()
                    };
                    for path in &to_upload {
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, &s3_prefix,
                            &retry_cfg, &pending, &dead_letter_dir,
                        ).await {
                            tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                        }
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
                    let to_upload = {
                        let mut guard = pending.lock().await;
                        guard.drain()
                    };
                    for path in &to_upload {
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, &s3_prefix,
                            &retry_cfg, &pending, &dead_letter_dir,
                        ).await {
                            tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                        }
                    }
                    if !to_upload.is_empty() {
                        tracing::info!(count = to_upload.len(), "periodic drain complete");
                    }
                }

                // ── SIGTERM / SIGINT → graceful drain and exit ──
                _ = sigterm.recv() => {
                    tracing::warn!("SIGTERM received, draining pending files before exit");
                    let to_upload = {
                        let mut guard = pending.lock().await;
                        guard.drain()
                    };
                    for path in &to_upload {
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, &s3_prefix,
                            &retry_cfg, &pending, &dead_letter_dir,
                        ).await {
                            tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                        }
                    }
                    tracing::warn!(count = to_upload.len(), "SIGTERM drain complete, exiting");
                    break;
                }

                _ = sigint.recv() => {
                    tracing::warn!("SIGINT received, draining pending files before exit");
                    let to_upload = {
                        let mut guard = pending.lock().await;
                        guard.drain()
                    };
                    for path in &to_upload {
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, &s3_prefix,
                            &retry_cfg, &pending, &dead_letter_dir,
                        ).await {
                            tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                        }
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

                // ── Periodic scan: rediscover untouched files ──
                _ = periodic_scan.tick() => {
                    tracing::info!("periodic scan timer fired");
                    let scan_filter = filter.lock().await;
                    match run_initial_scan(
                        &watch_root,
                        &scan_filter,
                        &s3,
                        &s3_prefix,
                        &inode_cache,
                    ).await {
                        Ok(stats) => tracing::info!(?stats, "periodic scan complete"),
                        Err(e) => tracing::error!(%e, "periodic scan failed"),
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
    ///
    /// If the file is stable (100ms dual-stat check passes), log at INFO with size.
    /// If still changing, still add to pending — the periodic drain will upload
    /// it on the next cycle when it may have settled.
    async fn debounce_and_queue(path: &std::path::Path, pending: &Arc<Mutex<PersistentPending>>) {
        let debouncer = crate::ebpf::debounce::Debouncer::new();
        match debouncer.check_stable(path) {
            Ok(Some(stable)) => {
                tracing::info!(
                    path = %stable.path.display(),
                    size = stable.size,
                    "file stable, queuing for upload"
                );
                pending.lock().await.insert(&stable.path);
            }
            Ok(None) => {
                // File still changing — queue anyway.  The periodic drain
                // (every 30s in standalone, 10min in Nomad) will upload
                // it when the file has likely settled.
                tracing::debug!(path = %path.display(), "file still changing, queued for deferred upload");
                pending.lock().await.insert(path);
            }
            Err(e) => {
                tracing::error!(path = %path.display(), %e, "debounce failed");
            }
        }
    }

    /// Upload a single file through the full pipeline with retry+backoff.
    ///
    /// On failure, retries up to `retry_cfg.max_attempts` with exponential
    /// backoff. Files exceeding max attempts are moved to the dead-letter queue.
    /// On success, removes the file from the pending set.
    ///
    /// Returns `Ok(())` on success, `Err(last_error)` if all retries exhausted.
    async fn upload_file(
        s3: &VerifiedS3Backend,
        path: &std::path::Path,
        watch_root: &std::path::Path,
        prefix: &str,
        retry_cfg: &RetryConfig,
        pending: &Arc<Mutex<PersistentPending>>,
        dead_letter_dir: &std::path::Path,
    ) -> Result<(), String> {
        let mut last_error = String::new();

        for attempt in 1..=retry_cfg.max_attempts {
            let result = Self::upload_file_once(s3, path, watch_root, prefix).await;

            match result {
                Ok(()) => {
                    // Remove from pending set — file is safely in S3
                    pending.lock().await.remove(path);
                    return Ok(());
                }
                Err(e) => {
                    last_error = e;
                    if attempt < retry_cfg.max_attempts {
                        let delay = crate::upload::retry::backoff_delay(retry_cfg, attempt);
                        tracing::warn!(
                            path = %path.display(),
                            attempt,
                            max = retry_cfg.max_attempts,
                            delay_ms = delay.as_millis(),
                            error = %last_error,
                            "upload failed, retrying after backoff"
                        );
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        // All retries exhausted — move to dead-letter queue
        let entry = DeadLetter {
            original_path: path.to_path_buf(),
            attempts: retry_cfg.max_attempts,
            last_error: last_error.clone(),
        };
        if let Err(e) = write_dead_letter(dead_letter_dir, &entry) {
            tracing::error!(%e, path = %path.display(), "failed to write dead-letter entry");
        }
        // Keep in pending set — operator can re-trigger later
        Err(last_error)
    }

    /// Single upload attempt (no retry). Returns Ok(()) or Err(description).
    async fn upload_file_once(
        _s3: &VerifiedS3Backend,
        path: &std::path::Path,
        watch_root: &std::path::Path,
        prefix: &str,
    ) -> Result<(), String> {
        crate::metrics::UPLOAD_TOTAL.inc();

        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown.db");

        // Build S3 key: {prefix}/{relative_path}/{file_name}
        let s3_key = {
            let rel = path.strip_prefix(watch_root).unwrap_or(path);
            let rel_parent = rel.parent().and_then(|p| p.to_str()).unwrap_or("");
            let prefix_clean = prefix.trim_matches('/');
            if rel_parent.is_empty() {
                format!("{}/{}", prefix_clean, file_name)
            } else {
                format!(
                    "{}/{}/{}",
                    prefix_clean,
                    rel_parent.trim_matches('/'),
                    file_name
                )
            }
        };

        // Open and stat the file
        let std_file = std::fs::File::open(path)
            .map_err(|e| format!("failed to open {}: {e}", path.display()))?;
        let file_size = std_file
            .metadata()
            .map_err(|e| format!("failed to stat {}: {e}", path.display()))?;
        let file_size = file_size.len();

        let file_fd = crate::fd::FileFd::from_file(std_file);

        // Stage 1: WAL checkpoint
        let checkpointed = crate::upload::pipeline::UploadPipeline::new(
            file_fd,
            file_size,
            s3_key.clone(),
            path.to_path_buf(),
        )
        .wal_checkpoint()
        .map_err(|e| format!("WAL checkpoint: {e}"))?;

        // Stage 2: Presign
        let presigned = checkpointed
            .presign(_s3)
            .await
            .map_err(|e| format!("S3 presign: {e}"))?;

        // Stage 3: Connect
        let connected = presigned
            .connect("localhost", 9000)
            .await
            .map_err(|e| format!("TCP connect: {e}"))?;

        // Stage 4: Write header + sendfile body
        let (header_written, sock) = connected
            .write_header(None)
            .map_err(|e| format!("header write: {e}"))?;

        let body_sent = header_written
            .sendfile_body(&sock)
            .map_err(|e| format!("sendfile: {e}"))?;

        // Stage 5: Shutdown + read response
        match body_sent.shutdown_and_read(sock) {
            Ok(outcome) if outcome.is_success() => {
                tracing::info!(s3_key, status = outcome.status_code, etag = ?outcome.etag(), "upload succeeded");
                crate::metrics::UPLOAD_BYTES_TOTAL.inc_by(file_size as f64);
                Ok(())
            }
            Ok(outcome) => {
                let msg = format!("HTTP {}", outcome.status_code);
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                Err(msg)
            }
            Err(e) => {
                let msg = format!("shutdown/read: {e}");
                crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
                Err(msg)
            }
        }
    }
}

/// Scan watch_root recursively, upload matching files, and fill the inode cache.
///
/// Litestream-style: discovers files at any depth under watch_root.
/// Existing files get a baseline upload; the InodeCache is populated
/// so subsequent BPF events resolve instantly (O(1)).
async fn run_initial_scan(
    watch_root: &std::path::Path,
    filter: &FileFilter,
    s3: &VerifiedS3Backend,
    s3_prefix: &str,
    inode_cache: &std::sync::Arc<InodeCache>,
) -> Result<ScanStats> {
    let mut stats = ScanStats::default();
    let mut dirs = vec![watch_root.to_path_buf()];

    while let Some(dir) = dirs.pop() {
        let entries = match tokio::fs::read_dir(&dir).await {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut entries = tokio_stream::wrappers::ReadDirStream::new(entries);

        while let Some(entry) = tokio_stream::StreamExt::next(&mut entries).await {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            let path = entry.path();

            let meta = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(_) => continue,
            };

            if meta.is_dir() && !meta.is_symlink() {
                dirs.push(path);
            } else if meta.is_file() && filter.should_monitor(&path) {
                stats.found += 1;

                // Fill cache
                use std::os::unix::fs::MetadataExt;
                let dev = meta.dev();
                let ino = meta.ino();
                inode_cache.insert(dev, ino, path.clone()).await;

                // Baseline upload
                HoardReady::upload_file(s3, &path, watch_root, s3_prefix).await;
                stats.uploaded += 1;
            }
        }
    }

    Ok(stats)
}

/// Statistics from an initial or periodic scan.
#[derive(Debug, Default, Clone)]
pub struct ScanStats {
    pub found: usize,
    pub uploaded: usize,
}
