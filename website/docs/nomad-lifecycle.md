# Nomad Volume Lifecycle

This page covers the complete lifecycle of a Hoard-backed volume in
a Nomad cluster, from first deployment through migration, including
failure modes, edge cases, and recovery procedures.

## Architecture Overview

```
┌─────────────────────────────────────────────────────────────┐
│                        Nomad Cluster                        │
│                                                             │
│  Node A                          Node B                     │
│  ┌──────────────────┐            ┌──────────────────┐       │
│  │ hoard daemon     │            │ hoard daemon     │       │
│  │  (eBPF watcher)  │            │  (eBPF watcher)  │       │
│  │  ┌──────┐        │            │  ┌──────┐        │       │
│  │  │S3    │────────┼──S3───────┼──│S3    │        │       │
│  │  │upload│        │            │  │upload│        │       │
│  │  └──────┘        │            │  └──────┘        │       │
│  └──────────────────┘            └──────────────────┘       │
│         ▲                               │                   │
│         │ eBPF hook                     │ prestart restore  │
│         ▼                               ▼                   │
│  ┌──────────────┐              ┌──────────────┐             │
│  │ app task     │───migrates──▶│ app task     │             │
│  │ (writes db)  │              │ (reads db)   │             │
│  └──────────────┘              └──────────────┘             │
└─────────────────────────────────────────────────────────────┘
```

## Lifecycle Stages

### 1. CREATE — First Deployment

```
Nomad places alloc on Node A
  │
  ├─► prestart hook: hoard nomad-restore
  │     ├─ mc ls backup/{job}/ → empty (first deploy)
  │     └─ --if-empty → skip restore
  │
  ├─► app task starts with fresh/empty data directory
  │
  └─► hoard daemon meta refresh (≤10s)
        ├─ detects nomad-hoard_enabled volumes
        ├─ scans alloc directory
        ├─ registers volume in registry
        └─ begins eBPF monitoring + periodic upload
```

**Job spec (first deploy snippet):**

```hcl
job "my-db" {
  meta {
    hoard_enabled  = "true"
    hoard_match    = "data/*.db"
    hoard_prefix   = "backup/my-db"
    hoard_class    = "sqlite"
  }

  group "db" {
    task "hoard-restore" {
      lifecycle {
        hook    = "prestart"
        sidecar = false   # ← task waits for restore to complete
      }
      driver = "raw_exec"
      config {
        command = "/usr/local/bin/hoard"
        args = ["nomad-restore", "--if-empty"]
      }
    }

    task "app" {
      driver = "raw_exec"
      config {
        command = "/usr/bin/my-app"
        args    = ["--data", "${NOMAD_ALLOC_DIR}/data"]
      }
    }
  }
}
```

### 2. RUN — Continuous Backup

```
┌──────────────────────────────────────────────────────┐
│                   RUN Phase                           │
│                                                      │
│  app writes file ──► eBPF hook fires                  │
│      (vfs_write)        │                             │
│                         ▼                             │
│                    inode resolution                    │
│                         │                             │
│                         ▼                             │
│                    500ms debounce                      │
│                         │                             │
│                         ▼                             │
│                    pending set (SQLite-backed)         │
│                         │                             │
│              ┌──────────┴───────────┐                 │
│              ▼                      ▼                 │
│     periodic drain (60s)    HTTP /flush trigger       │
│              │                      │                 │
│              └──────────┬───────────┘                 │
│                         ▼                             │
│                  S3 upload                            │
│              (sendfile + SigV4)                       │
│                         │                             │
│                         ▼                             │
│                   ETag verify                         │
│                                                      │
│  Meta refresh (10s): confirms alloc still local       │
└──────────────────────────────────────────────────────┘
```

**Key invariants during RUN:**

| Property | Mechanism |
|----------|-----------|
| Every write captured | eBPF `fentry/vfs_write` hooks the VFS layer |
| No lost writes on crash | Pending set is SQLite WAL (crash-safe) |
| Deduplication | Inode→(dev,ino,size,mtime) comparison |
| Zero-copy upload | `sendfile(2)` from page cache to TCP socket |
| S3 eventual consistency | ETag MD5 verification after PUT |
| Graceful retry | Exponential backoff: 1s→60s, 5 retries |
| Dead letter | Files exceeding retries logged, alert fired |

### 3. STOP / MIGRATE — Graceful Drain

```
Nomad decides to stop alloc on Node A
  │
  ├─► SIGTERM to app task
  │     └─ app exits (or is killed after kill_timeout)
  │
  ├─► poststop hook: curl -X POST http://localhost:9090/nomad-drain
  │     ├─ wait 500ms (eBPF debounce window)
  │     ├─ trigger flush of all pending files
  │     ├─ poll pending count until 0 (or timeout)
  │     └─ return {"status":"ok","pending":0}
  │
  ├─► Nomad cleans up alloc directory
  │
  └─► Nomad places replacement alloc on Node B
        └─ enters CREATE phase (prestart restore)
```

**Job spec (full lifecycle snippet):**

```hcl
job "my-db" {
  meta {
    hoard_enabled  = "true"
    hoard_match    = "data/*.db"
    hoard_prefix   = "backup/my-db"
  }

  group "db" {
    task "app" {
      driver = "raw_exec"

      # Give poststop time to drain (default 5s → 60s for large files)
      kill_timeout = "60s"

      config {
        command = "/usr/bin/my-app"
        args    = ["--data", "${NOMAD_ALLOC_DIR}/data"]
      }

      # ── Poststop: drain hoard before Nomad cleans up directory ──
      lifecycle {
        hook    = "poststop"
        sidecar = false
      }
    }

    task "hoard-restore" {
      lifecycle {
        hook    = "prestart"
        sidecar = false
      }
      driver = "raw_exec"
      config {
        command = "/usr/local/bin/hoard"
        args = ["nomad-restore", "--if-empty"]
      }
    }

    # ── Post-drain: tell hoard to flush synchronously ──
    task "hoard-drain" {
      lifecycle {
        hook    = "poststop"
        sidecar = false
      }
      driver = "raw_exec"
      config {
        command = "/bin/sh"
        args = ["-c", <<EOF
          # Wait for eBPF debounce, then drain
          curl -sS -X POST http://localhost:9090/nomad-drain?timeout=60000
        EOF
        ]
      }
    }
  }
}
```

!!! warning "Poststop order"
    Nomad runs poststop hooks in the order tasks are defined. Place
    `hoard-drain` **after** `app` so the main task has exited before
    drain starts.

### 4. DRIFT — Automatic Recovery

When the daemon can't rely on poststop hooks (node crash, Nomad API
delay, manual intervention), the **drift detector** catches orphaned
volumes:

```
Meta refresh (10s) on Node A:
  │
  ├─► Fetch list of hoard-enabled allocs from Nomad API
  │
  ├─► Compare with previous snapshot:
  │     old: { nomad-my-db, nomad-my-cache, static-fs }
  │     new: { nomad-my-cache, static-fs }
  │
  ├─► nomad-my-db disappeared → DRIFT DETECTED
  │     ├─ Final scan of alloc directory
  │     ├─ Upload any remaining files
  │     ├─ Remove from volume registry
  │     └─ Log: drifted=1
  │
  └─► Next tick: drifted=0, registry stable
```

**Drift log example:**

```json
{"message":"Nomad alloc drifted: draining removed volume","name":"nomad-my-db","base_dir":"/opt/nomad/data/alloc/abc123/alloc"}
{"message":"drift drain scan complete","stats":{"found":3,"uploaded":3},"dir":"/opt/nomad/data/alloc/abc123/alloc"}
{"message":"volume registry updated with Nomad meta","total":1,"nomad":0,"drifted":1}
```

## Edge Cases & Failure Modes

### Node Crash (No Poststop)

| Situation | Outcome |
|-----------|---------|
| Node A crashes | No poststop runs. Alloc dir NOT cleaned. |
| Pending DB survives | SQLite WAL is crash-safe. |
| Hoard restarts on Node A | Recovers pending set, uploads unsent files. |
| Nomad reschedules to Node B | Prestart restore pulls from S3. |
| Data loss window | Files written after last successful upload. |

**Mitigation**: Set `hoard_drain_interval` low (e.g. 10s) to minimize
the window. For zero-loss, use synchronous replication (outside hoard's
scope).

### Split Brain (Should Not Occur)

```
Nomad guarantees: replacement alloc created only after old alloc stopped.
If a bug causes two allocs simultaneously:

  Node A: alloc-1 running, hoard uploading
  Node B: alloc-2 running, hoard uploading
       │                    │
       └──── S3 ───────────┘
               │
        Both upload to same prefix
        Last PUT wins (atomic overwrite)
        No corruption — but version conflict possible
```

**Mitigation**: Enable S3 object versioning. Hoard's ETag verification
detects no corruption; version conflicts require application-level
reconciliation.

### S3 Unavailable

| Phase | Behavior |
|-------|----------|
| Prestart restore | `mc ls` fails → prestart fails → alloc rescheduled |
| Upload | 5× exponential backoff (1s→60s) → dead letter queue |
| Dead letter | File logged. App keeps running (non-blocking). |
| Recovery | Operator runs `hoard restore` manually after S3 returns |

### Large File Drain Timeout

```
File size: 1 GB
Bandwidth: 100 Mbps
Upload time: ~80 seconds
kill_timeout: 60s

  → Nomad kills the poststop task at 60s
  → Upload incomplete → S3 object NOT committed
  → File lost (alloc directory cleaned)
```

**Mitigation:**
1. Set `kill_timeout` > expected upload time
2. Estimate: `kill_timeout = (largest_file_bytes × 8) / (bandwidth_bps) + 30s`
3. Formula: `kill_timeout = (1GB × 8) / 100Mbps + 30s = 110s`

### Restore on First Deploy

```
First deployment:
  Prestart: hoard nomad-restore --if-empty
  → S3 prefix "backup/my-db/" is empty
  → No files to restore
  → App starts with fresh data
  → Hoard begins uploading
```

**`--if-empty` guard**: If directory is non-empty (e.g. persistent
volume mount), restore is skipped. Without this flag, restore
overwrites existing files.

### Restore After Migration

```
Node B gets replacement alloc:
  Prestart: hoard nomad-restore
  → S3 has files from Node A's uploads
  → Downloads: data/app.db, data/wal.db, ...
  → App starts with data from last upload
  → Data freshness: ≤ drain_interval (10s default)
```

## Control Plane Reference

### `hoard nomad-restore`

```bash
# Auto-detect everything from Nomad environment
hoard nomad-restore

# Explicit prefix, override S3 bucket
hoard nomad-restore --prefix backup/my-db --s3-bucket my-bucket

# Dry run: show what would be restored
hoard nomad-restore --dry-run

# Force overwrite existing files
hoard nomad-restore --force
```

**Environment variables read:**

| Variable | Purpose |
|----------|---------|
| `HOARD_S3_ENDPOINT` | S3 endpoint URL |
| `HOARD_S3_BUCKET` | S3 bucket name |
| `HOARD_S3_ACCESS_KEY` | S3 access key |
| `HOARD_S3_SECRET_KEY` | S3 secret key |
| `NOMAD_ALLOC_DIR` | Restore destination |
| `NOMAD_META_hoard_prefix` | S3 prefix (from job meta) |
| `HOARD_S3_PREFIX` | Fallback S3 prefix |
| `NOMAD_JOB_NAME` | Last-resort prefix: `backup/{job}` |

### `POST /nomad-drain`

```bash
# Default 30s timeout
curl -X POST http://localhost:9090/nomad-drain

# Custom timeout (ms)
curl -X POST "http://localhost:9090/nomad-drain?timeout=60000"
```

**Response:**

```json
{"status":"ok","pending":0,"wait_ms":1234}
{"status":"timeout","pending":3,"wait_ms":30000}
```

### `POST /flush`

Fire-and-forget drain trigger. Best for scripts that don't need to wait:

```bash
curl -X POST http://localhost:9090/flush
# → {"status":"ok","message":"flush triggered"}
```

### `GET /health`

```bash
curl http://localhost:9090/health
# → {"status":"ok","pending":0,"dead_letter":0}
# → {"status":"degraded","pending":15,"dead_letter":2}
```

## Meta Key Reference

All keys use **underscore** format (Nomad HCL does not support dots):

| Key | Type | Required | Default | Description |
|-----|------|----------|---------|-------------|
| `hoard_enabled` | `"true"` / `"false"` | Yes | — | Enable hoard for this job |
| `hoard_match` | glob | Yes | — | File pattern (e.g. `data/*.db`) |
| `hoard_prefix` | string | No | `backup/{job}` | S3 key prefix |
| `hoard_class` | string | No | — | Storage class hint |
| `hoard_ttl` | duration | No | `7d` | Object lifecycle (e.g. `30d`, `90d`) |
| `hoard_extensions` | string | No | — | File extensions (e.g. `.db,.sqlite`) |

## Timing Reference

| Timer | Default | Purpose |
|-------|---------|---------|
| eBPF debounce | 500ms | Wait for writes to settle before queuing |
| Upload retry backoff | 1s → 60s | Exponential, 5 retries |
| Periodic drain | 60s (standalone) / 600s (Nomad) | Flush pending set |
| Meta refresh | 10s | Poll Nomad API for volume changes |
| GC cycle | 1h | S3 object lifecycle cleanup |
| Poststop drain timeout | 30s | HTTP `/nomad-drain?timeout=` |
| `kill_timeout` (recommended) | 60-120s | Nomad window before SIGKILL |

## Migration Checklist

When migrating an existing app to hoard + Nomad:

1. [ ] Add meta keys to Nomad job spec
2. [ ] Add `hoard nomad-restore --if-empty` as prestart task
3. [ ] Add `/nomad-drain` curl as poststop task
4. [ ] Set `kill_timeout` based on largest file size
5. [ ] Verify hoard daemon running on all nodes
6. [ ] Test: submit job, verify S3 upload
7. [ ] Test: stop job, verify drain + restart on new node
8. [ ] Test: kill -9 node, verify drift recovery
9. [ ] Monitor: check `/health` after each migration
10. [ ] Set S3 object versioning (governance)
