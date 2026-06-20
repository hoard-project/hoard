# Changelog

## v1.0.0-beta.1 (2026-06-20)

First beta release. Production-validated on dual-node Nomad cluster.

### Core
- eBPF dual VFS hook: `fentry/vfs_write` + `fentry/generic_perform_write`
- Zero-copy `sendfile(2)` upload to S3-compatible storage
- SigV4 signing (pure Rust, ~120 lines)
- BTF CO-RE: one BPF object, any kernel ≥ 5.5
- `pread(2)` TOCTOU-safe MD5 verification (v0.6.0+)

### Modes
- **standalone**: Unix socket control + periodic drain (30s) + SIGHUP hot-reload
- **nomad**: SSE lifecycle events + periodic drain (10min) + meta auto-discovery

### Configuration
- v1: flat TOML with env var expansion
- v2: StorageClass + Volume model with `conf.d/` hot-reload
- Per-volume: TTL, retries, extensions, compression, S3 prefix, on-stop/on-delete

### Operations
- `hoard ctl status|flush|restore` control commands
- Prometheus metrics (8 counters/gauges/histograms + 5 alert rules)
- Health endpoint (`/health` → `{"status":"ok"}` or `{"status":"degraded"}`)
- SQLite-backed pending set with crash recovery
- Exponential backoff retry (5×, base 1s, max 60s) + dead-letter queue
- GC: S3 object lifecycle (TTL-based)
- WAL checkpoint for SQLite files (TRUNCATE→PASSIVE backoff)

### Robustness
- SIGTERM/SIGINT graceful drain of pending uploads
- SIGHUP config hot-reload (v2)
- `hoard-atomic` helper: atomic file writer for overwrite-heavy workloads
- Recursive directory scan: catches files created but never written to
- Inode→path cache (4096 entries, LRU) for hot-path performance

### CI/CD
- CI: fmt + clippy (0 warnings) + test (49/49) + build (x86_64 + aarch64)
- Release: GitHub Release with binary + BPF object + sha256 checksums
- CodeQL + Dependabot security scanning

### Documentation
- GitHub Pages with just-the-docs theme (10 pages)
- AI-friendly: table-driven config schema, typed CLI flags, wire protocol
- Architecture deep dive with data flow diagram
- Operations guide: restore, metrics, health, troubleshooting

---

## v0.6.5 (2026-06-20)
- 33/33 modules `#![deny(unsafe_code)]`
- Clippy default: 0 warnings
- Release sha256 files: filename only (no path prefix)
- CI: all green (fmt + clippy + test + build ×2)

## v0.6.4 (2026-06-20)
- Release workflow fix: BPF object auto-packaging
- sha256 filename-only fix

## v0.6.2 (2026-06-20)
- BPF object included in GitHub Release assets
- CI: e2e job attempt (withdrawn — manual testing preferred)

## v0.6.1 (2026-06-19)
- `pread_md5` TOCTOU-safe ETag verification
- v2 StorageClass + Volume configuration model
- `conf.d/` hot-reload
- Nomad meta auto-discovery
- Verified: 795 uploads, 0 dead letters, 0 ETag mismatches (3-round stress test)

## v0.5.x (2026-06-18)
- Renamed from Guardian to Hoard
- Dual BPF hook architecture
- WAL checkpoint for SQLite
- Pending DB persistence (SQLite, WAL mode)

## v0.4.x (2026-06-17)
- Initial Guardian prototype
- Single BPF hook (`fentry/vfs_write`)
- Basic S3 upload with sendfile
