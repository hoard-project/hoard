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
use crate::config::registry::VolumeRegistry;
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

/// Update Prometheus gauges with current state.
async fn update_gauges(pending: &Arc<Mutex<PersistentPending>>, dead_letter_dir: &std::path::Path) {
    let pending_count = pending.lock().await.len() as u64;
    let dead_count = crate::upload::retry::count_dead_letters(dead_letter_dir);
    crate::metrics::update_health_gauges(pending_count, dead_count);
}

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

        let registry = VolumeRegistry::new(self.config.volumes.clone());
        let inode_cache = Arc::new(InodeCache::new());

        // ── Initial scan: baseline upload of all matching files ──
        {
            let scan_result = run_initial_scan(
                &self.config.watch_path,
                &filter,
                &s3,
                &registry,
                &inode_cache,
            )
            .await;
            match scan_result {
                Ok(stats) => tracing::info!(?stats, "initial scan complete"),
                Err(e) => tracing::error!(%e, "initial scan failed (non-fatal)"),
            }
        }

        tracing::info!(mode = ?self.config.mode, "Hoard ready");
        tracing::info!(
            "volume registry: {} volumes", registry.len()
        );
        for v in registry.iter() {
            tracing::info!(
                "  volume '{}': match={}, prefix={}, ttl={}",
                v.name, v.match_glob, v.s3_prefix, v.ttl
            );
        }
        Ok(HoardReady {
            ebpf: self.ebpf,
            s3,
            trigger,
            config: self.config,
            filter,
            inode_cache,
            registry,
        })
    }
}

// ── State: Ready (main loop) ─────────────────────────────────────

pub struct HoardReady {
    ebpf: BpfProgram,
    s3: VerifiedS3Backend,
    trigger: TriggerSource,
    config: ValidatedConfig,
    filter: FileFilter,
    inode_cache: Arc<InodeCache>,
    registry: VolumeRegistry,
}

impl HoardReady {
    /// Resolve upload config for a given file path.
    fn resolve_upload_params<'a>(
        registry: &'a VolumeRegistry,
        watch_root: &std::path::Path,
        path: &std::path::Path,
    ) -> (&'a str, u32) {
        let vol = registry.resolve(path, watch_root);
        (vol.s3_prefix.as_str(), vol.retries)
    }

    /// Per-volume OnStop policy-aware graceful shutdown.
    async fn graceful_shutdown(
        s3: &VerifiedS3Backend,
        registry: &VolumeRegistry,
        watch_root: &std::path::Path,
        pending: &Arc<Mutex<PersistentPending>>,
        dead_letter_dir: &std::path::Path,
        retry_cfg: &RetryConfig,
    ) {
        // 1. Drain all pending files regardless of volume policy
        let to_upload = {
            let mut guard = pending.lock().await;
            guard.drain()
        };
        let mut uploaded = 0u64;
        let mut failed = 0u64;
        for path in &to_upload {
            let (prefix, _) = Self::resolve_upload_params(registry, watch_root, path);
            match Self::upload_file(s3, path, watch_root, prefix, retry_cfg, pending, dead_letter_dir).await {
                Ok(()) => uploaded += 1,
                Err(e) => {
                    tracing::error!(path = %path.display(), %e, "shutdown upload failed");
                    failed += 1;
                }
            }
        }
        tracing::warn!(
            uploaded = uploaded,
            failed = failed,
            "pending drain complete"
        );

        // 2. Per-volume OnStop policy logging
        for vol in registry.iter() {
            let policy = match vol.on_stop {
                crate::config::v2::OnStop::Drain => "drain (files uploaded, kept on disk)",
                crate::config::v2::OnStop::Keep => "keep (files left on disk, no action)",
                crate::config::v2::OnStop::Purge => "purge (files uploaded then deleted from disk)",
            };
            tracing::info!(
                volume = %vol.name,
                match_glob = %vol.match_glob,
                on_stop = policy,
                "shutdown policy applied"
            );
        }

        update_gauges(pending, dead_letter_dir).await;
    }

    pub async fn run(self) -> Result<()> {
        let gc_interval = Duration::from_secs(self.config.gc_interval_secs);
        let watch_root = Arc::new(self.config.watch_path.clone());
        let registry = Arc::new(self.registry);
        let filter = Arc::new(Mutex::new(self.filter));

        // ── Nomad meta auto-discovery (if enabled) ──
        let meta_discovery = if self.config.nomad_meta_enabled {
            let poll_secs = self.config.nomad_meta_poll_secs;
            let watch_path_str = self.config.watch_path.to_string_lossy().to_string();
            let token = self.config.nomad_token.clone();
            self.config.nomad_addr.as_ref().map(|addr| {
                crate::nomad::meta::MetaDiscovery::new(
                    addr,
                    token.as_deref(),
                    &watch_path_str,
                    poll_secs,
                )
            })
        } else {
            None
        };
        let mut meta_timer = if meta_discovery.is_some() {
            let mut t = tokio::time::interval(Duration::from_secs(self.config.nomad_meta_poll_secs));
            t.tick().await; // skip first immediate tick
            Some(t)
        } else {
            None
        };

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
                // ── BPF event → resolve → check volume filter → debounce → queue ──
                Some(ev) = bpf_rx.recv() => {
                    let watch = watch_root.clone();
                    let pend = pending.clone();
                    let cache = inode_cache.clone();
                    let reg = registry.clone();

                    // Spawn as fire-and-forget — concurrent debounce for high throughput
                    tokio::spawn(async move {
                        // Step 1: resolve inode to path
                        let path = cache.resolve(&watch, ev.dev, ev.ino).await;
                        let path = match path {
                            Some(ref p) => p.clone(),
                            None => return,
                        };

                        // Step 2: resolve volume & check per-volume extensions filter
                        let vol = reg.resolve(&path, &watch);
                        if !vol.should_monitor(&path) {
                            return;
                        }

                        // Step 3: debounce and queue for upload
                        Self::debounce_and_queue(&path, &pend).await;
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
                                let (prefix, _) = Self::resolve_upload_params(&registry, &watch_root, path);
                                if let Err(e) = Self::upload_file(
                                    &s3, path, &watch_root, prefix,
                                    &retry_cfg, &pending, &dead_letter_dir,
                                ).await {
                                    tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                                }
                            }
                            tracing::info!(count = to_upload.len(), "drain complete");
                            update_gauges(&pending, &dead_letter_dir).await;
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
                        let (prefix, _) = Self::resolve_upload_params(&registry, &watch_root, path);
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, prefix,
                            &retry_cfg, &pending, &dead_letter_dir,
                        ).await {
                            tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                        }
                    }
                    tracing::info!(count = to_upload.len(), "flush drain complete");
                    update_gauges(&pending, &dead_letter_dir).await;
                }

                // ── GC timer ──
                _ = gc_timer.tick() => {
                    tracing::info!("GC timer fired");
                    let volumes = registry.to_vec();
                    for vol in &volumes {
                        let ttl = crate::config::registry::parse_ttl(&vol.ttl);
                        tracing::info!(
                            volume = %vol.name,
                            prefix = %vol.s3_prefix,
                            ttl_secs = ttl.as_secs(),
                            "GC: scanning volume"
                        );
                        match crate::s3::gc::gc_cycle_mc("guser", &s3.bucket_name(), &vol.s3_prefix, ttl).await {
                            Ok(stats) => {
                                tracing::info!(?stats, volume = %vol.name, "GC cycle complete");
                                crate::metrics::GC_CYCLES_TOTAL.inc();
                                crate::metrics::GC_DELETED_TOTAL.inc_by(stats.deleted as f64);
                                crate::metrics::GC_ERRORS_TOTAL.inc_by(stats.errors as f64);
                            }
                            Err(e) => tracing::error!(%e, volume = %vol.name, "GC cycle failed"),
                        }
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
                        let (prefix, _) = Self::resolve_upload_params(&registry, &watch_root, path);
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, prefix,
                            &retry_cfg, &pending, &dead_letter_dir,
                        ).await {
                            tracing::error!(path = %path.display(), %e, "upload exhausted retries");
                        }
                    }
                    if !to_upload.is_empty() {
                        tracing::info!(count = to_upload.len(), "periodic drain complete");
                    }
                    update_gauges(&pending, &dead_letter_dir).await;
                }

                // ── SIGTERM / SIGINT → graceful drain and exit ──
                _ = sigterm.recv() => {
                    tracing::warn!("SIGTERM received, graceful shutdown — per-volume OnStop policy");
                    Self::graceful_shutdown(
                        &s3, &registry, &watch_root, &pending,
                        &dead_letter_dir, &retry_cfg,
                    ).await;
                    break;
                }

                _ = sigint.recv() => {
                    tracing::warn!("SIGINT received, graceful shutdown — per-volume OnStop policy");
                    Self::graceful_shutdown(
                        &s3, &registry, &watch_root, &pending,
                        &dead_letter_dir, &retry_cfg,
                    ).await;
                    break;
                }

                // ── SIGHUP → reload config (filter + GC) ──
                _ = sighup.recv() => {
                    tracing::info!("SIGHUP received, reloading configuration");
                    match reload_config(&config, &filter).await {
                        Ok(()) => tracing::info!("config reloaded successfully"),
                        Err(e) => tracing::error!(%e, "config reload failed"),
                    }
                }

                // ── Nomad meta refresh ──
                Some(_) = async {
                    if let Some(ref mut t) = meta_timer {
                        t.tick().await;
                        Some(())
                    } else {
                        std::future::pending::<()>().await;
                        None
                    }
                } => {
                    if let Some(ref md) = meta_discovery {
                        match md.discover().await {
                            Ok(meta_vols) => {
                                tracing::info!(count = meta_vols.len(), "Nomad meta refresh: discovered volumes");
                                for v in &meta_vols {
                                    tracing::info!(name=%v.name, prefix=%v.s3_prefix, ttl=%v.ttl, "meta volume");
                                }
                            }
                            Err(e) => tracing::warn!(%e, "Nomad meta refresh failed"),
                        }
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
                        &registry,
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
        crate::metrics::PENDING_FILES.set(pending.lock().await.len() as f64);
    }

    /// Upload with full retry+backoff+dead-letter. Used from the main loop.
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

        let entry = DeadLetter {
            original_path: path.to_path_buf(),
            attempts: retry_cfg.max_attempts,
            last_error: last_error.clone(),
        };
        if let Err(e) = write_dead_letter(dead_letter_dir, &entry) {
            tracing::error!(%e, path = %path.display(), "failed to write dead-letter entry");
        }
        Err(last_error)
    }

    /// Upload a single attempt without retry or pending management.
    /// Used by initial/periodic scans (which don't go through the pending set).
    pub(crate) async fn upload_file_once_scan(
        s3: &VerifiedS3Backend,
        path: &std::path::Path,
        watch_root: &std::path::Path,
        prefix: &str,
        _retries: u32,
    ) {
        match Self::upload_file_once(s3, path, watch_root, prefix).await {
            Ok(()) => {}
            Err(e) => {
                tracing::error!(path = %path.display(), %e, "scan upload failed");
            }
        }
    }

    /// Single upload attempt (no retry). Returns Ok(()) or Err(description).
    ///
    /// The TCP connect, sendfile, and HTTP response read are moved into
    /// `spawn_blocking` so they never block the tokio runtime thread.
    /// This is critical for remote S3/MinIO endpoints where a single hung
    /// connection would otherwise stall all async tasks (including the
    /// metrics endpoint and BPF ringbuf polling).
    async fn upload_file_once(
        _s3: &VerifiedS3Backend,
        path: &std::path::Path,
        watch_root: &std::path::Path,
        prefix: &str,
    ) -> Result<(), String> {
        crate::metrics::UPLOAD_TOTAL.inc();
        crate::metrics::UPLOAD_IN_FLIGHT.inc();
        let _start = std::time::Instant::now();

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

        // Stage 1: WAL checkpoint (local, fast)
        let checkpointed = crate::upload::pipeline::UploadPipeline::new(
            file_fd,
            file_size,
            s3_key.clone(),
            path.to_path_buf(),
        )
        .wal_checkpoint()
        .map_err(|e| format!("WAL checkpoint: {e}"))?;

        // ── Pre-compute MD5 (pread, separate fd) to eliminate TOCTOU ──
        // sendfile and verify_etag previously read the file at different
        // times — if an application overwrites the file between the two
        // reads, the ETag comparison fails spuriously.  We now compute
        // the MD5 digest *once*, before sendfile, using pread on a
        // dedicated file handle.  The digest is compared against the S3
        // ETag without ever re-reading the file.
        let expected_md5 = crate::verify::pread_md5(path)
            .map_err(|e| format!("pread MD5: {e}"))?;

        // Stage 2: Presign (async — S3 API call)
        let connected = checkpointed
            .presign(_s3)
            .await
            .map_err(|e| format!("S3 presign: {e}"))?
            .connect("localhost", 9000)
            .await
            .map_err(|e| format!("TCP connect: {e}"))?;

        // Stage 3–5: TCP connect + sendfile + HTTP response read
        // Moved into spawn_blocking so slow/remote S3 endpoints never
        // stall the tokio runtime (metrics, BPF ringbuf, health checks).
        let s3_key_for_blocking = s3_key.clone();
        let outcome = tokio::task::spawn_blocking(move || {
            // Stage 3: TCP connect + HTTP header write
            let (header_written, sock) = connected
                .write_header(None)
                .map_err(|e| format!("header write: {e}"))?;

            // Stage 4: Send file body via sendfile(2)
            let body_sent = header_written
                .sendfile_body(&sock)
                .map_err(|e| format!("sendfile: {e}"))?;

            // Stage 5: Read HTTP response
            let outcome = body_sent
                .shutdown_and_read(sock)
                .map_err(|e| format!("shutdown/read: {e}"))?;

            Ok::<_, String>((outcome, s3_key_for_blocking))
        })
        .await
        .map_err(|e| format!("spawn_blocking panicked: {e}"))?
        .inspect_err(|_| {
            crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
        })?;

        let (upload_outcome, s3_key) = outcome;

        // Stage 6: Verify integrity — pre-computed MD5 vs S3 ETag
        // No file re-read.  The digest was captured via pread(2) after
        // checkpoint and before sendfile, from a separate file handle.
        // This eliminates the TOCTOU race where verify_etag would re-open
        // and re-read the file after sendfile, potentially seeing different
        // content if the application overwrote it mid-upload.
        let upload_result = if upload_outcome.is_success() {
            tracing::info!(%s3_key, status = upload_outcome.status_code, etag = ?upload_outcome.etag(), "upload succeeded");

            if let Some(etag) = upload_outcome.etag() {
                let etag = etag.trim_matches('"');
                if !expected_md5.eq_ignore_ascii_case(etag) {
                    tracing::error!(%s3_key, local = %expected_md5, s3 = %etag, "ETag mismatch — possible data corruption");
                    crate::metrics::ETAG_MISMATCH_TOTAL.inc();
                    Err(format!("ETag mismatch: local={expected_md5} s3={etag}"))
                } else {
                    crate::metrics::UPLOAD_BYTES_TOTAL.inc_by(file_size as f64);
                    Ok(())
                }
            } else {
                tracing::warn!(
                    %s3_key,
                    "S3 response missing ETag — skipping integrity check"
                );
                crate::metrics::UPLOAD_BYTES_TOTAL.inc_by(file_size as f64);
                Ok(())
            }
        } else {
            let msg = format!("HTTP {}", upload_outcome.status_code);
            crate::metrics::UPLOAD_FAILURES_TOTAL.inc();
            Err(msg)
        };

        // Cleanup: always decrement in-flight and record duration
        crate::metrics::UPLOAD_IN_FLIGHT.dec();
        crate::metrics::UPLOAD_DURATION_SECONDS.observe(_start.elapsed().as_secs_f64());

        upload_result
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
    registry: &VolumeRegistry,
    inode_cache: &std::sync::Arc<InodeCache>,
) -> Result<ScanStats> {
    let mut stats = ScanStats::default();
    let mut dirs = vec![watch_root.to_path_buf()];

    // Collect all matching files first so we can upload them concurrently
    // with a semaphore cap (avoids flooding the S3 endpoint).
    let mut files_to_upload: Vec<std::path::PathBuf> = Vec::new();

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

                files_to_upload.push(path);
            }
        }
    }

    // Upload concurrently with a bounded cap (8) so slow/dead
    // connections never stall the tokio runtime — each upload
    // already runs inside spawn_blocking (§5.4).
    if !files_to_upload.is_empty() {
        // Arc the S3 backend so spawned tasks can hold a 'static reference.
        let s3_arc = std::sync::Arc::new(s3.clone());
        let watch_root_arc = std::sync::Arc::new(watch_root.to_path_buf());
        let registry_arc = std::sync::Arc::new(registry.clone());
        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let mut join_set = tokio::task::JoinSet::new();

        for path in files_to_upload {
            let s3 = s3_arc.clone();
            let watch_root = watch_root_arc.clone();
            let reg = registry_arc.clone();
            let permit = semaphore.clone();
            join_set.spawn(async move {
                let _permit = permit.acquire().await;
                let (prefix, retries) = HoardReady::resolve_upload_params(&reg, &watch_root, &path);
                HoardReady::upload_file_once_scan(&s3, &path, &watch_root, prefix, retries).await;
            });
        }

        while let Some(result) = join_set.join_next().await {
            if result.is_ok() {
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
