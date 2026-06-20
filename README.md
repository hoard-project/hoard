# Hoard — eBPF file-change replication daemon

Zero-copy file backup to S3, hooked at the VFS layer. No application
changes needed.

```mermaid
flowchart LR
    A[write] --> B[eBPF dual-hook]
    B --> C[RingBuffer]
    C --> D[inode → path]
    D --> E[filter + debounce]
    E --> F[pending set]
    F --> G[periodic drain]
    G --> H[sendfile → S3]
```

## Key features

- **Dual VFS hook**: `fentry/vfs_write` + `fentry/generic_perform_write`
  catches every buffered write on ext4, tmpfs, btrfs, xfs
- **Zero-copy upload**: `sendfile(2)` from page cache straight to TLS socket
- **SQLite auto-detect**: WAL checkpoint for `.db` files; transparent for others
- **S3 key preserves directory structure**: `{prefix}/{relpath}/{filename}`
- **BTF CO-RE**: one BPF object, any kernel ≥ 5.5
- **Dual-mode**: standalone (control socket) or Nomad system job (SSE events)
- **v2 StorageClass + Volume model**: per-volume TTL, retries, compression,
  S3 routing, on-stop/on-delete lifecycle

## Quickstart

```bash
# Download
curl -sL https://github.com/hoard-project/hoard/releases/latest/download/hoard-x86_64 \
  -o /usr/local/bin/hoard
curl -sL https://github.com/hoard-project/hoard/releases/latest/download/hoard-x86_64.bpf.o \
  -o /usr/lib/hoard/hoard.bpf.o
chmod +x /usr/local/bin/hoard

# Run
HOARD_MODE=standalone \
HOARD_WATCH_ROOT=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=http://127.0.0.1:9000 \
HOARD_S3_BUCKET=my-backups \
HOARD_S3_ACCESS_KEY=xxx \
HOARD_S3_SECRET_KEY=yyy \
  hoard
```

→ **[Full documentation](https://hoard-project.github.io/hoard/)**

## Docs

| Section | Content |
|---------|---------|
| [Quickstart](https://hoard-project.github.io/hoard/quickstart) | Install, run, verify |
| [Configuration](https://hoard-project.github.io/hoard/configuration) | v1 + v2 (StorageClass/Volume) full reference |
| [Operations](https://hoard-project.github.io/hoard/operations) | Restore, metrics, health, troubleshooting |
| [Architecture](https://hoard-project.github.io/hoard/architecture) | Kernel → userspace deep dive |
| [Nomad](https://hoard-project.github.io/hoard/nomad) | System job deployment |
| [CLI Reference](https://hoard-project.github.io/hoard/reference/cli) | Every flag and subcommand |
| [Control Socket API](https://hoard-project.github.io/hoard/reference/control-socket) | Wire protocol for automation |

## Requirements

| Component | Minimum |
|-----------|---------|
| Linux kernel | 5.5 (BPF trampoline + BTF) |
| Rust | 1.82 |
| S3 backend | any S3-compatible (MinIO, AWS, Garage, …) |

## License

GPL-3.0

## Status

Production-ready. v0.6.5. Verified on Linux 6.1 & 6.12.
