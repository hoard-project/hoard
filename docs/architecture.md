---
title: Architecture
nav_order: 5
---

# Architecture

Hoard is a Rust daemon that hooks the Linux VFS layer via eBPF to detect
file writes, then replicates changed files to S3 using zero-copy
`sendfile(2)`.

---

## Data flow

```
┌──────────────────────────────────────────────────────────────┐
│                        KERNEL SPACE                          │
│                                                              │
│  write(2) ──► VFS layer ──► fentry/vfs_write                │
│                  │           fentry/generic_perform_write    │
│                  │                                           │
│                  └──► BPF RingBuffer ──► userspace poll      │
│                                                              │
└──────────────────────────────────────────────────────────────┘
                           │
┌──────────────────────────▼───────────────────────────────────┐
│                      USERSPACE                               │
│                                                              │
│  RingBuf poll ──► inode→path resolution ──► debounce 100ms   │
│                                               │              │
│                                    ┌──────────┘              │
│                                    ▼                         │
│                              filter (extensions, glob)       │
│                                    │                         │
│                                    ▼                         │
│                              pending set (HashSet<PathBuf>)  │
│                                    │                         │
│                    ┌───────────────┼───────────────┐         │
│                    ▼               ▼               ▼         │
│              periodic drain   ctl flush      SIGTERM drain   │
│              (30s / 10min)   (Unix socket)   (graceful exit) │
│                    │               │               │         │
│                    └───────┬───────┘───────────────┘         │
│                            ▼                                 │
│              ┌─────────────────────────┐                     │
│              │  per-file upload loop   │                     │
│              │  1. open file (O_RDONLY)│                     │
│              │  2. WAL checkpoint (*)  │                     │
│              │  3. pread(2) MD5        │                     │
│              │  4. sendfile(2) → S3    │                     │
│              │  5. verify ETag         │                     │
│              └─────────────────────────┘                     │
│                                                              │
└──────────────────────────────────────────────────────────────┘
```

{: .note }
(*) WAL checkpoint only for SQLite files (`.db`, `.sqlite`, `.sqlite3`).
Detected by reading the first 16 bytes: `SQLite format 3\0`.

---

## BPF hooks

Hoard installs **two** `fentry` BPF programs at load time:

| Hook | Catches | Example workloads |
|------|---------|-------------------|
| `fentry/vfs_write` | `pwrite64`, SQLite WAL writes | Databases |
| `fentry/generic_perform_write` | `write(2)`, buffered I/O | `echo`, `dd`, `cat > file`, app logs |

Both hooks extract `(inode, dev_t)` from `struct file` and push to a shared
`BPF_MAP_TYPE_RINGBUF`. No file path resolution happens in BPF — that's
deferred to userspace for safety.

**CO-RE**: The BPF object is compiled once with full `vmlinux.h` and uses
BTF CO-RE relocations. One object works on any kernel ≥ 5.5.

### Why two hooks?

`vfs_write` alone misses buffered writes that go through the page cache
without calling `vfs_write`. The `generic_perform_write` hook catches
these. Together they cover all write paths.

---

## Userspace pipeline

### 1. inode → path resolution

BPF gives us `(inode, dev_t)`. Userspace resolves this to a filesystem
path via `/proc/self/fd` traversal or by scanning `/proc/<pid>/fd/` for
open file handles.

A `RwLock<HashMap<InodeKey, PathBuf>>` cache (4096 entries, LRU eviction)
avoids repeated `/proc` scans for hot inodes.

### 2. Debounce

A single `write()` syscall can trigger multiple BPF events (write + metadata
update). Debounce merges events on the same inode within a 100ms window.

### 3. Filter

Two-stage filtering:
- **Extension**: file extension must match `filter.extensions` (or volume-level override)
- **Exclude**: glob patterns in `filter.exclude` skip matching paths

Non-matching files are silently dropped. No error, no dead-letter.

### 4. Pending set

A `HashSet<PathBuf>` accumulates paths between drains. Duplicate writes
to the same file are coalesced (last-write-wins within a drain window).

**Persistence**: the pending set is backed by a SQLite database
(`pending.db`, WAL mode, `PRAGMA synchronous=NORMAL`). On crash restart,
Hoard recovers the pending set from SQLite and continues uploading.

### 5. Periodic drain

Every 30 seconds (standalone) or 10 minutes (Nomad), the pending set is
drained: each file is opened, uploaded, and removed from the set.

The drain is also triggered by:
- `hoard ctl flush` (Unix socket, standalone only)
- `SIGTERM` / `SIGINT` (graceful shutdown)

### 6. Upload pipeline (per file)

```
open(O_RDONLY) → WAL checkpoint? → pread(2) MD5 → sendfile(2) → verify ETag
```

**WAL checkpoint**: for SQLite files, runs `PRAGMA wal_checkpoint(TRUNCATE)`
before opening the file for upload. This ensures the WAL is flushed to the
main database file. Passive mode is tried first, then TRUNCATE. On failure,
the upload continues with the current state — stale data is better than no
data.

**pread_md5**: opens an independent file descriptor (`O_RDONLY`) and uses
`pread(2)` to compute MD5 at the current file offset. This is TOCTOU-safe
because the fd reads at a fixed snapshot, even if the file is concurrently
modified. This MD5 is compared against the S3 ETag after upload.

**sendfile**: `sendfile(2)` copies data from the page cache directly to
the TLS socket fd. Zero userspace buffer. The kernel handles the copy
entirely in kernel space.

**ETag verification**: after upload, the S3 response includes an ETag
(which is the MD5 of the uploaded object for unencrypted PUTs). This is
compared against the pre-computed MD5. Mismatch → retry.

### 7. Retry

Exponential backoff: base 1s, max 60s, up to 5 retries (configurable via
`max_upload_retries`). After all retries exhausted, file is moved to
`dead_letter_dir` and counted in `hoard_dead_letter_files`.

---

## Modes

### Standalone mode

- Triggers: periodic timer (30s) + Unix socket (`hoard ctl flush`)
- Lifecycle: `SIGTERM`/`SIGINT` → drain pending → exit
- Hot-reload: `SIGHUP` → reload config + conf.d

### Nomad mode

- Triggers: Nomad SSE event stream + periodic timer (10min)
- Lifecycle: Nomad alloc lifecycle events (started, stopping, stopped)
- Meta discovery: polls Nomad API for `hoard.*` job meta, synthesizes virtual volumes
- On alloc stop: drain pending, then exit

---

## Safety guarantees

### Unsafe code

All unsafe code is isolated in `src/ffi.rs` (~120 lines). Every other
module starts with `#![deny(unsafe_code)]`. Each unsafe block has a
`// SAFETY:` comment documenting its preconditions.

### Memory

- BPF programs are verified by the kernel's eBPF verifier before loading.
- BPF maps use `BPF_MAP_TYPE_RINGBUF` (lock-free, single-producer single-consumer).
- Userspace uses `tokio` async runtime with a dedicated BPF poll task.

### File integrity

- `pread(2)` on independent fd eliminates TOCTOU between read and sendfile.
- MD5 computed pre-upload, verified against S3 ETag post-upload.
- Dead-letter queue captures failed uploads for manual inspection.

---

## Performance

| Operation | Mechanism | Cost |
|-----------|-----------|------|
| Write detection | BPF fentry (kernel trampoline) | ~1µs |
| File copy to S3 | `sendfile(2)` (kernel DMA) | ~disk I/O bound |
| MD5 computation | `pread(2)` | ~disk read (cached if file in page cache) |
| S3 upload | TLS over TCP | ~network bound |

**Typical throughput**: 100-200 MB/s on local MinIO, network-bound on remote S3.
