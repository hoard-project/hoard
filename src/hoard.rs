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

use crate::config::registry::VolumeRegistry;
use crate::config::{Mode, ValidatedConfig};
use crate::ebpf::resolve::InodeCache;
use crate::ebpf::{BpfProgram, FileFilter};
use crate::pending::PersistentPending;
use crate::s3::{S3Backend, VerifiedS3Backend};
use crate::trigger::TriggerSource;
use crate::upload::retry::{write_dead_letter, DeadLetter, RetryConfig};
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Update metrics gauges with current state.
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
        tracing::info!("volume registry: {} volumes", registry.len());
        for v in registry.iter() {
            tracing::info!(
                "  volume '{}': match={}, prefix={}, ttl={}",
                v.name,
                v.match_glob,
                v.s3_prefix,
                v.ttl
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
    /// Returns (s3_prefix, retries, root, compression).'s base_dir
    /// or the global watch_root.
    fn resolve_upload_params(
        registry: &VolumeRegistry,
        watch_root: &std::path::Path,
        path: &std::path::Path,
    ) -> (String, u32, std::path::PathBuf, Option<String>) {
        let (vol, root) = registry.resolve_with_root(path, watch_root);
        (vol.s3_prefix, vol.retries, root, vol.compression)
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
            let (prefix, _, _, comp) = Self::resolve_upload_params(registry, watch_root, path);
            match Self::upload_file(
                s3,
                path,
                watch_root,
                &prefix,
                comp.as_deref(),
                retry_cfg,
                pending,
                dead_letter_dir,
            )
            .await
            {
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
            tracing::info!(
                meta_enabled = true,
                poll_secs = self.config.nomad_meta_poll_secs,
                nomad_addr = ?self.config.nomad_addr,
                "Nomad meta auto-discovery enabled"
            );
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
            tracing::info!("Nomad meta auto-discovery disabled");
            None
        };
        // Meta discovery channel: background timer -> select!
        let (meta_tx, mut meta_rx) = tokio::sync::mpsc::channel::<()>(1);
        if meta_discovery.is_some() {
            let poll_secs = self.config.nomad_meta_poll_secs;
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(Duration::from_secs(poll_secs));
                ticker.tick().await; // skip first immediate tick
                loop {
                    ticker.tick().await;
                    let _ = meta_tx.send(()).await;
                }
            });
        }

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

        // ── Start metrics server ──
        {
            let metrics_addr = config.metrics_addr.clone();
            tracing::info!(addr = %metrics_addr, "Metrics endpoint starting");
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
        // (recursive: pick up new databases in subdirectories).
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
                                let (prefix, _, _, comp) = Self::resolve_upload_params(&registry, &watch_root, path);
                                if let Err(e) = Self::upload_file(
                                    &s3, path, &watch_root, &prefix,
                                    comp.as_deref(),
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
                        let (prefix, _, _, comp) = Self::resolve_upload_params(&registry, &watch_root, path);
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, &prefix,
                            comp.as_deref(),
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
                            on_delete = ?vol.on_delete,
                            "GC: scanning volume"
                        );
                        match crate::s3::gc::gc_cycle_mc("guser", s3.bucket_name(), &vol.s3_prefix, ttl).await {
                            Ok(mut stats) => {
                                // OnDelete::Delete — clean up orphaned S3 objects
                                if vol.on_delete == crate::config::v2::OnDelete::Delete {
                                    match crate::s3::gc::gc_orphan_cleanup(
                                        "guser",
                                        s3.bucket_name(),
                                        &vol.s3_prefix,
                                        &watch_root,
                                    ) {
                                        Ok(orphans) => {
                                            stats.orphans_deleted = orphans;
                                            if orphans > 0 {
                                                tracing::info!(orphans, volume = %vol.name, "GC: orphan cleanup");
                                            }
                                        }
                                        Err(e) => tracing::error!(%e, volume = %vol.name, "GC: orphan cleanup failed"),
                                    }
                                }
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
                        let (prefix, _, _, comp) = Self::resolve_upload_params(&registry, &watch_root, path);
                        if let Err(e) = Self::upload_file(
                            &s3, path, &watch_root, &prefix,
                            comp.as_deref(),
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

                // ── SIGHUP → reload config (filter + volumes) ──
                _ = sighup.recv() => {
                    tracing::info!("SIGHUP received, reloading configuration");
                    match reload_config(&config, &filter, &registry).await {
                        Ok(()) => tracing::info!("config reloaded successfully"),
                        Err(e) => tracing::error!(%e, "config reload failed"),
                    }
                }

                // ── Nomad meta refresh ──
                _ = meta_rx.recv() => {
                    if let Some(ref md) = meta_discovery {
                        match md.discover().await {
                            Ok(meta_vols) => {
                                // Detect drift: which nomad volumes existed before but are gone now?
                                let old_nomad: Vec<_> = registry.to_vec()
                                    .into_iter()
                                    .filter(|v| v.name.starts_with("nomad-"))
                                    .collect();
                                let new_names: std::collections::HashSet<_> = meta_vols
                                    .iter()
                                    .map(|v| v.name.clone())
                                    .collect();

                                let mut drift_drained = 0usize;
                                for old_vol in &old_nomad {
                                    if !new_names.contains(&old_vol.name) {
                                        tracing::info!(
                                            name=%old_vol.name,
                                            base_dir=?old_vol.base_dir,
                                            "Nomad alloc drifted: draining removed volume"
                                        );
                                        drift_drained += 1;
                                        // Trigger final scan of the departed alloc dir.
                                        if let Some(ref base) = old_vol.base_dir {
                                            if base.is_dir() {
                                                let s3_drain = s3.clone();
                                                let reg_drain = registry.clone();
                                                let filter_drain = filter.clone();
                                                let cache_drain = inode_cache.clone();
                                                let root_drain = watch_root.clone();
                                                let base_drain = base.clone();
                                                tokio::spawn(async move {
                                                    let f = filter_drain.lock().await;
                                                    match run_initial_scan(&root_drain, &f, &s3_drain, &reg_drain, &cache_drain).await {
                                                        Ok(stats) => tracing::info!(?stats, dir=%base_drain.display(), "drift drain scan complete"),
                                                        Err(e) => tracing::error!(%e, dir=%base_drain.display(), "drift drain scan failed"),
                                                    }
                                                });
                                            }
                                        }
                                    }
                                }

                                // Merge: static config volumes + discovered meta volumes
                                let mut merged = meta_vols;
                                for v in registry.to_vec() {
                                    if !v.name.starts_with("nomad-") {
                                        merged.push(v);
                                    }
                                }
                                let new_count = merged.iter().filter(|v| v.name.starts_with("nomad-")).count();
                                registry.reload(merged);
                                tracing::info!(
                                    total = registry.len(),
                                    nomad = new_count,
                                    drifted = drift_drained,
                                    "volume registry updated with Nomad meta"
                                );

                                // Trigger a one-shot scan for newly discovered volumes.
                                let s3_scan = s3.clone();
                                let reg_scan = registry.clone();
                                let filter_scan = filter.clone();
                                let cache_scan = inode_cache.clone();
                                let root_scan = watch_root.clone();
                                tokio::spawn(async move {
                                    let f = filter_scan.lock().await;
                                    match run_initial_scan(&root_scan, &f, &s3_scan, &reg_scan, &cache_scan).await {
                                        Ok(stats) => tracing::info!(?stats, "meta discovery scan complete"),
                                        Err(e) => tracing::error!(%e, "meta discovery scan failed"),
                                    }
                                });
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

/// Reload filter + volumes from TOML config + conf.d on SIGHUP.
async fn reload_config(
    config: &Arc<ValidatedConfig>,
    filter: &Arc<Mutex<FileFilter>>,
    registry: &Arc<VolumeRegistry>,
) -> Result<()> {
    let cfg_path = config
        .config_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no config path — SIGHUP reload requires --config"))?;

    // Read and parse v2 config once
    let raw = std::fs::read_to_string(cfg_path).context("reading v2 config for SIGHUP")?;
    let expanded = crate::config::env::expand_env(&raw);
    let mut v2_config: crate::config::v2::ConfigV2 =
        toml::from_str(&expanded).context("parsing v2 config on SIGHUP")?;

    // ── Reload file filter from v2 defaults ──
    let has_filter =
        v2_config.defaults.extensions.is_some() || v2_config.defaults.exclude.is_some();
    if has_filter {
        let patterns: Vec<String> = v2_config
            .defaults
            .extensions
            .clone()
            .map(|exts| exts.iter().map(|e| format!("*.{e}")).collect())
            .unwrap_or_else(|| vec!["*".into()]);
        let excludes: Vec<String> = v2_config.defaults.exclude.clone().unwrap_or_default();
        let new_filter = FileFilter::new(config.watch_path.clone(), &patterns, &excludes)
            .context("failed to rebuild filter from reloaded config")?;
        let _old = std::mem::replace(&mut *filter.lock().await, new_filter);
        tracing::info!("filter reloaded from config");
    }

    // ── Load conf.d directories ──
    let conf_dirs: Vec<_> = v2_config.hoard.conf_dirs.clone();
    for dir in &conf_dirs {
        let resolved_dir = expand_path(dir, cfg_path);
        if resolved_dir.is_dir() {
            crate::config::v2::load_conf_dir(&resolved_dir, &mut v2_config)
                .context("reloading conf.d")?;
        }
    }

    // ── Reload volume registry ──
    let new_vols =
        crate::config::v2::resolve_volumes(&v2_config).context("resolving volumes on SIGHUP")?;
    registry.reload(new_vols);
    tracing::info!(count = registry.len(), "volume registry reloaded");

    Ok(())
}

/// Expand config-relative paths like "conf.d" → "/etc/hoard/conf.d"
fn expand_path(dir: &str, config_path: &Path) -> std::path::PathBuf {
    let p = std::path::Path::new(dir);
    if p.is_absolute() {
        p.to_path_buf()
    } else if let Some(parent) = config_path.parent() {
        parent.join(p)
    } else {
        p.to_path_buf()
    }
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
        compression: Option<&str>,
        retry_cfg: &RetryConfig,
        pending: &Arc<Mutex<PersistentPending>>,
        dead_letter_dir: &std::path::Path,
    ) -> Result<(), String> {
        let mut last_error = String::new();

        for attempt in 1..=retry_cfg.max_attempts {
            let result = Self::upload_file_once(s3, path, watch_root, prefix, compression).await;

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
        compression: Option<&str>,
        _retries: u32,
    ) {
        match Self::upload_file_once(s3, path, watch_root, prefix, compression).await {
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
    /// This is critical for remote S3 endpoints where a single hung
    /// connection would otherwise stall all async tasks (including the
    /// metrics endpoint and BPF ringbuf polling).
    async fn upload_file_once(
        s3: &VerifiedS3Backend,
        path: &std::path::Path,
        watch_root: &std::path::Path,
        prefix: &str,
        compression: Option<&str>,
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

        // ── Compressed upload path (zstd) ──
        if compression == Some("zstd") {
            let raw = std::fs::read(path)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            let compressed = zstd::encode_all(&raw[..], 3)
                .map_err(|e| format!("zstd compress: {e}"))?;
            let compressed_key = format!("{s3_key}.zst");
            let compressed_md5 = format!("{:x}", md5::compute(&compressed));

            let etag = s3
                .put_bytes(&compressed_key, &compressed)
                .await
                .map_err(|e| format!("compressed upload: {e}"))?;

            if !compressed_md5.eq_ignore_ascii_case(&etag) {
                crate::metrics::ETAG_MISMATCH_TOTAL.inc();
                crate::metrics::UPLOAD_IN_FLIGHT.dec();
                return Err(format!(
                    "ETag mismatch: local={compressed_md5} s3={etag}"
                ));
            }

            crate::metrics::UPLOAD_IN_FLIGHT.dec();
            return Ok(());
        }

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
        let expected_md5 = crate::verify::pread_md5(path).map_err(|e| format!("pread MD5: {e}"))?;

        // Stage 2: Presign (async — S3 API call)
        let connected = checkpointed
            .presign(s3)
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
/// Recursive: discovers files at any depth under watch_root.
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

    // Also scan per-volume base_dirs (e.g. Nomad alloc directories).
    let mut volume_base_dirs: Vec<std::path::PathBuf> = Vec::new();
    for v in registry.iter() {
        if let Some(ref base) = v.base_dir {
            if base.is_dir() {
                dirs.push(base.clone());
                volume_base_dirs.push(base.clone());
            }
        }
    }

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
            } else if meta.is_file() {
                // Accept file if it passes the global watch filter, OR
                // if it lives under a volume-specific base_dir (e.g. Nomad alloc).
                let is_under_volume_dir = volume_base_dirs.iter().any(|b| path.starts_with(b));
                let accepted = filter.should_monitor(&path)
                    || (is_under_volume_dir && registry.iter().any(|v| v.should_monitor(&path)));
                if accepted {
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
                let (prefix, retries, root, comp) =
                    HoardReady::resolve_upload_params(&reg, &watch_root, &path);
                HoardReady::upload_file_once_scan(&s3, &path, &root, &prefix, comp.as_deref(), retries).await;
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
