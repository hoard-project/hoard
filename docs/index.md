---
title: Hoard
nav_order: 1
permalink: /
---

# Hoard — eBPF zero-copy file replication to S3

Hoard watches file writes at the kernel VFS layer via eBPF and replicates
changed files to S3-compatible storage using `sendfile(2)` zero-copy.

{: .note }
Applications need **zero changes**. Hoard hooks below the filesystem.

## Architecture (10 seconds)

```
write() → BPF fentry (vfs_write + generic_perform_write)
       → RingBuffer → inode→path → debounce 100ms
       → pending set → periodic drain 30s
       → sendfile(2) → S3 (SigV4)
```

**Key property**: files are copied from page cache directly to the TLS
socket. No userspace buffer, no `read()` syscall. One `sendfile` per upload.

## When to use

| Use case | Fit |
|----------|-----|
| SQLite backup (Litestream-style but S3-native) | ✅ auto WAL checkpoint |
| Log / JSON / CSV / Parquet shipping | ✅ transparent pass-through |
| Nomad cluster backup (system job, one per node) | ✅ SSE lifecycle |
| Large file replication (ISO, tar, DB dumps) | ✅ sendfile zero-copy |
| Real-time sync (sub-second latency) | ❌ debounce 100ms + drain 30s |

## Quick numbers

| Metric | Value |
|--------|-------|
| Binary size (stripped) | 4.2 MB |
| BPF object | 808 KB (CO-RE, portable) |
| Runtime RSS | ~30 MB |
| Kernel requirement | ≥ 5.5 (BPF trampoline) |
| Filesystems | ext4, tmpfs, btrfs, xfs |
| S3 backends | MinIO, AWS S3, Garage, any S3-compatible |
| Rust MSRV | 1.82 |

## Modes

| Mode | Trigger | Drain interval | Use case |
|------|---------|---------------|----------|
| `standalone` | Unix socket (`hoard ctl flush`) | 30 s | single-node, VPS, docker |
| `nomad` | Nomad SSE events + periodic timer | 10 min | cluster system job |

Both modes share the same core pipeline (BPF → filter → pending → S3).

## Quickstart

```bash
HOARD_MODE=standalone \
HOARD_WATCH_ROOT=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=http://127.0.0.1:9000 \
HOARD_S3_BUCKET=my-backups \
HOARD_S3_ACCESS_KEY=xxx \
HOARD_S3_SECRET_KEY=yyy \
  hoard
```

→ See [Quickstart](quickstart) for full install and first-run guide.

## Next steps

| I want to… | Go to |
|------------|-------|
| Configure volumes, TTL, S3 backends | [Configuration](configuration) |
| Restore files, check health, view metrics | [Operations](operations) |
| Deploy on a Nomad cluster | [Nomad](nomad) |
| Understand the kernel internals | [Architecture](architecture) |
| Look up a CLI flag | [CLI Reference](reference/cli) |
| Script against the control socket | [Control Socket API](reference/control-socket) |
