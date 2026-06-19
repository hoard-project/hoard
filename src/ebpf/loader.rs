//! BPF program loading and RingBuffer management.
//!
//! Uses the `aya` crate to load CO-RE BPF programs and manage
//! the RingBuffer for user-space event delivery.
//!
//! Hook: `fentry/vfs_write` + `fentry/generic_perform_write`
//! with bpf_probe_read_kernel for safe struct field access
//! across all filesystems.

#![deny(unsafe_code)]

use anyhow::{Context, Result};
use std::time::Duration;

/// An event from the BPF RingBuffer: a file that has been
/// quiet for ≥ 2 seconds after its last write.
#[derive(Debug, Clone, Copy)]
pub struct DevInoEvent {
    /// Device number
    pub dev: u64,
    /// Inode number
    pub ino: u64,
    /// Monotonic timestamp of the last write (nanoseconds)
    pub timestamp_ns: u64,
}

/// Loaded BPF program with attached tracepoints and RingBuffer.
///
/// Polling the RingBuffer is done through this struct directly,
/// avoiding complex lifetime issues with separating the RingBuf.
pub struct BpfProgram {
    inner: aya::Ebpf,
    ringbuf: Option<aya::maps::RingBuf<aya::maps::MapData>>,
}

impl Drop for BpfProgram {
    fn drop(&mut self) {
        // Explicitly drop ringbuf first to release any ongoing
        // kernel-side operations before unloading programs.
        self.ringbuf.take();
        // aya::Ebpf::drop() detaches all programs and unloads maps.
        tracing::info!("BPF programs detached, maps unloaded");
    }
}

impl BpfProgram {
    /// Clean up stale BPF filesystem pins from previous runs.
    ///
    /// These survive process death (especially SIGKILL) and can
    /// cause ringbuf contention or map-reuse errors on restart.
    /// Handles both directories and individual pinned files.
    pub fn cleanup_stale() {
        use std::fs;

        fn remove_any(path: &str) {
            let p = std::path::Path::new(path);
            if !p.exists() {
                return;
            }
            let result = if p.is_dir() {
                fs::remove_dir_all(p)
            } else {
                fs::remove_file(p)
            };
            match result {
                Ok(()) => tracing::info!(path, "cleaned stale BPF pin"),
                Err(e) => tracing::warn!(path, %e, "failed to clean stale BPF pin"),
            }
        }

        // Clean known hoard pin paths — both directories and files.
        let paths = ["/sys/fs/bpf/hoard_maps", "/sys/fs/bpf/hoard"];
        for path in &paths {
            remove_any(path);
        }

        // Also clean any orphan pinnings left by aya's default naming.
        if let Ok(entries) = fs::read_dir("/sys/fs/bpf") {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("minimal_") {
                    remove_any(&format!("/sys/fs/bpf/{name_str}"));
                }
            }
        }
    }

    /// Load the compiled BPF bytecode and attach all programs.
    ///
    /// If the BPF object file is missing or empty, gracefully degrades
    /// to an empty program — Hoard will still run (log + trigger hooks
    /// work) but won't intercept filesystem events.
    pub async fn load() -> Result<Self> {
        use aya::Ebpf;

        // Clean stale pins before loading to avoid ringbuf contention.
        Self::cleanup_stale();

        let bpf_path = {
            let installed = "/usr/lib/hoard/hoard.bpf.o";
            std::env::var("HOARD_BPF_OBJECT")
                .ok()
                .or_else(|| {
                    if std::path::Path::new(installed).exists() {
                        Some(installed.to_string())
                    } else {
                        option_env!("HOARD_BPF_OBJECT_BUILD").map(|s| s.to_string())
                    }
                })
                .unwrap_or_else(|| installed.to_string())
        };

        // If the BPF object file is missing, degrade gracefully
        match std::fs::metadata(&bpf_path) {
            Ok(m) if m.len() == 0 => {
                tracing::warn!(path = %bpf_path, "BPF object is empty — eBPF disabled");
                return Ok(Self {
                    inner: Ebpf::load(&[])?,
                    ringbuf: None,
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                tracing::warn!(path = %bpf_path, "BPF object not found — eBPF disabled");
                return Ok(Self {
                    inner: Ebpf::load(&[])?,
                    ringbuf: None,
                });
            }
            Err(e) => return Err(e.into()),
        };

        let bpf_bytes = std::fs::read(&bpf_path)
            .with_context(|| format!("failed to read BPF object: {}", bpf_path))?;

        let mut bpf = Ebpf::load(&bpf_bytes).context("failed to load BPF program")?;

        // Multi-hook: vfs_write covers tmpfs/nfs/etc.
        // generic_perform_write covers write()/dd/echo on ext4/xfs/tmpfs.
        // __generic_file_write_iter may be inlined on newer kernels (6.12+).
        Self::attach_fentry(&mut bpf, "on_vfs_write", "vfs_write")?;

        if let Err(e) = Self::attach_fentry(
            &mut bpf,
            "on_generic_perform_write",
            "generic_perform_write",
        ) {
            tracing::warn!(%e, "generic_perform_write fentry not available — fallback to vfs_write only");
        };

        // Take ownership of the "events" ring buffer map.
        let ringbuf = {
            use aya::maps::RingBuf;
            let map = bpf
                .take_map("events")
                .ok_or_else(|| anyhow::anyhow!("'events' ring buffer map not found"))?;
            let rb =
                RingBuf::try_from(map).context("failed to create RingBuf from 'events' map")?;
            tracing::debug!("RingBuf initialized");
            Some(rb)
        };

        tracing::info!(path = %bpf_path, "eBPF programs loaded and attached (fentry)");
        Ok(Self {
            inner: bpf,
            ringbuf,
        })
    }

    fn attach_fentry(bpf: &mut aya::Ebpf, prog_name: &str, fn_name: &str) -> Result<()> {
        use aya::{programs::FEntry, Btf};

        let btf = Btf::from_sys_fs()
            .context("failed to load kernel BTF for fentry — ensure CONFIG_DEBUG_INFO_BTF=y")?;

        let prog: &mut FEntry = bpf
            .program_mut(prog_name)
            .with_context(|| format!("BPF program not found: {prog_name}"))?
            .try_into()?;
        prog.load(fn_name, &btf)?;
        prog.attach()?;
        Ok(())
    }

    /// Poll a single event from the RingBuffer.
    ///
    /// Returns `None` if the buffer is empty. Non-blocking.
    pub fn poll(&mut self, _timeout: Duration) -> Result<Option<DevInoEvent>> {
        let Some(ref mut ringbuf) = self.ringbuf else {
            return Ok(None);
        };

        match ringbuf.next() {
            Some(item) => {
                let slice: &[u8] = &item;
                if slice.len() >= 24 {
                    let dev = u64::from_ne_bytes(slice[0..8].try_into().expect("len checked"));
                    let ino = u64::from_ne_bytes(slice[8..16].try_into().expect("len checked"));
                    let timestamp_ns =
                        u64::from_ne_bytes(slice[16..24].try_into().expect("len checked"));
                    Ok(Some(DevInoEvent {
                        dev,
                        ino,
                        timestamp_ns,
                    }))
                } else {
                    tracing::warn!(len = slice.len(), "short BPF ringbuf event");
                    Ok(None)
                }
            }
            None => Ok(None),
        }
    }
}
