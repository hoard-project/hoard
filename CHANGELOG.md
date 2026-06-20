# Changelog

## v1.0.1 (2026-06-20)

Nomad integration complete: meta auto-discovery + alloc drift handling.

### Nomad Integration
- **Meta auto-discovery**: channel-driven Nomad API polling (timer → mpsc → select!)
- Nomad meta key format: underscore-separated (`hoard_enabled`) for HCL compatibility
- Per-volume `base_dir` support: `ResolvedVolume` now carries optional alloc directory
- `VolumeRegistry::resolve_with_root()`: tries volume base_dir before global watch_root
- `run_initial_scan`: walks volume-specific base_dirs, bypasses FileFilter for them
- Auto-scan trigger: one-shot scan fires immediately after meta volumes discovered

### Drift Handling
- Detects removed Nomad volumes by comparing old vs new meta set on each poll
- Triggers final drain scan of departed alloc directory before removing from registry
- Registry correctly converges to static volumes only after drift
- Drift metrics logged: `nomad`, `drifted` counts in registry update event

### Fixes
- Self node ID extraction: `/v1/agent/self` → `stats.client.node_id` (was `config.NodeID`)
- JobStub/Job/TaskGroup deserialization: individual `#[serde(rename)]` over `rename_all`
- Registry deduplication: `nomad-` prefix filter prevents volume accumulation

## v1.0.0 (2026-06-20)

First stable release. Production-validated on dual-node Nomad cluster.

### Core
- eBPF dual VFS hook: `fentry/vfs_write` + `fentry/generic_perform_write`
- Zero-copy `sendfile(2)` upload to S3-compatible storage
- SigV4 signing (pure Rust, ~120 lines)
- BTF CO-RE: one BPF object, any kernel ≥ 5.5
- `pread(2)` TOCTOU-safe MD5 verification

### Modes
- **standalone**: Unix socket control + periodic drain (30s) + SIGHUP hot-reload
- **nomad**: SSE lifecycle events + periodic drain (10min) + meta auto-discovery

### Configuration
- v1: flat TOML with env var expansion
- v2: StorageClass + Volume model with `conf.d/` hot-reload
- Per-volume: TTL, retries, extensions, compression, S3 prefix, on-stop/on-delete
- Env var overrides for all key settings

### Operations
- `hoard ctl status|flush|restore` control commands
- Observability metrics (8 counters/gauges/histograms + 5 alert rules)
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

### Code quality
- 33/33 modules `#![deny(unsafe_code)]` (100% coverage)
- 13 unsafe blocks, all with SAFETY comments
- Clippy default: 0 warnings
- 49/49 unit tests passing
- Full trademark de-branding: 28 comment references cleaned

### CI/CD
- CI: fmt + clippy (0 warnings) + test (49/49) + build (x86_64 + aarch64)
- Release: 8 assets per version (binary + BPF object + sha256, 2 arches)
- CodeQL + `cargo audit` + `cargo deny` on every PR
- OpenSSF Scorecard analysis (weekly)
- Dependabot for dependency updates

### Documentation
- 9-page GitHub Pages site (MkDocs Material)
- AI-friendly: table-driven config schema, typed CLI flags, wire protocol
- Architecture deep dive with Mermaid data flow diagram
- Operations guide: restore, metrics, health, troubleshooting

### Governance (v1.0.0)
- CODE_OF_CONDUCT.md (CNCF)
- CONTRIBUTING.md
- MAINTAINERS.md
- GOVERNANCE.md (BDFL + Maintainer model)
- CODEOWNERS
- Issue templates (bug + feature)
- Pull request template
- RELEASE.md

### IP Audit (v1.0.0)
- License: GPL-3.0, zero transitive GPL/AGPL contamination
- All 35 direct dependencies: MIT or Apache-2.0
- No embedded third-party code
- All trademark references removed from docs and comments

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
