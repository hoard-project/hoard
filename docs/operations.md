---
title: Operations
nav_order: 4
---

# Operations

Day-2 operations: restore, metrics, health checks, and troubleshooting.

---

## Control socket (`hoard ctl`)

The control socket is at `/run/hoard/<service>.sock` (standalone mode only).
Use `hoard ctl` to interact with a running daemon.

### `hoard ctl status <service>`

Returns JSON with daemon health.

```bash
hoard ctl status default
```

Response:

```json
{
  "service": "default",
  "mode": "standalone",
  "health": "ok",
  "pending_files": 0,
  "total_uploads": 1523,
  "dead_letter_files": 0,
  "uptime_secs": 86400
}
```

### `hoard ctl flush <service>`

Triggers an immediate drain of the pending set. All queued files are uploaded
to S3 synchronously.

```bash
hoard ctl flush default
# Flush complete: 3 files uploaded, 0 failed
```

---

## Restore

{: .important }
Restore uses the MinIO Client (`mc`). Install it first:
`curl -sL https://dl.min.io/client/mc/release/linux-amd64/mc -o /usr/local/bin/mc && chmod +x /usr/local/bin/mc`

### List available backups

```bash
hoard ctl restore default --list
```

### Restore all files

```bash
hoard ctl restore default \
  --target /tmp/restored
```

Files are decompressed (zstd) and placed under `--target` with their
original directory structure preserved.

### Restore a specific prefix

```bash
hoard ctl restore default \
  --target /tmp/restored \
  --prefix databases/postgres
```

### Dry-run

```bash
hoard ctl restore default --target /tmp/restored --dry-run
# Would restore 1523 objects (1.2 GB)
```

---

## Metrics (Prometheus)

Metrics are exposed at `http://<host>:9150/metrics` in Prometheus text format.

| Metric | Type | Description |
|--------|------|-------------|
| `hoard_uploads_total` | counter | Total successful uploads |
| `hoard_upload_bytes_total` | counter | Total bytes uploaded |
| `hoard_upload_errors_total` | counter | Failed uploads (after retries) |
| `hoard_dead_letter_files` | gauge | Files in dead-letter directory |
| `hoard_pending_files` | gauge | Files awaiting next drain |
| `hoard_bpf_events_total` | counter | BPF events received |
| `hoard_drain_duration_secs` | histogram | Drain execution time |
| `hoard_upload_duration_secs` | histogram | Per-file upload duration |

### Alerting rules

See [`contrib/prometheus/alerts.yml`]({{ site.baseurl }}/../contrib/prometheus/alerts.yml)
for pre-built Prometheus alert rules:

| Alert | Condition | Severity |
|-------|-----------|----------|
| `HoardDeadLetterFiles` | `hoard_dead_letter_files > 0` | warning |
| `HoardHighPendingFiles` | `hoard_pending_files > 100` | warning |
| `HoardUploadFailureRate` | error rate > 10% over 5min | critical |
| `HoardDown` | metrics endpoint unreachable | critical |

### Prometheus scrape config

```yaml
scrape_configs:
  - job_name: hoard
    static_configs:
      - targets: ['localhost:9150']
```

---

## Health check

```bash
curl http://127.0.0.1:9150/health
```

Response codes:

| HTTP status | Body | Meaning |
|-------------|------|---------|
| `200` | `{"status":"ok"}` | Healthy |
| `503` | `{"status":"degraded","dead_letter_files":3}` | Dead-letter files present |

Health is considered **degraded** when `hoard_dead_letter_files > 0`. This
means some files failed upload after all retries.

---

## Troubleshooting

### Files not appearing in S3

{: .note }
**Diagnosis steps, in order:**

1. **Check filter**: Does the file extension match `filter.extensions`?
   ```bash
   grep extensions /etc/hoard/hoard.toml
   ```
   BPF detects ALL writes, but hoard silently drops non-matching extensions.

2. **Check health**:
   ```bash
   curl http://127.0.0.1:9150/health
   ```

3. **Check pending queue**:
   ```bash
   hoard ctl status default | grep pending_files
   ```
   If > 0, files are queued. Wait for next drain (30s) or force flush:
   ```bash
   hoard ctl flush default
   ```

4. **Check dead letters**:
   ```bash
   ls /var/lib/hoard/dead-letter/
   ```
   If files exist here, uploads failed after 5 retries. Check S3 connectivity.

5. **Check logs** (if using systemd):
   ```bash
   journalctl -u hoard --since "5 minutes ago"
   ```

### BPF program not loading

```bash
# Check kernel version (needs ≥ 5.5)
uname -r

# Check BTF support
ls /sys/kernel/btf/vmlinux

# Check BPF program list (hoard should appear)
bpftool prog list | grep hoard
```

If BPF fails to load, Hoard falls back to periodic directory scanning
(every 30 minutes). This is a degraded but functional mode.

### Upload failures

```
Error: upload failed after 5 retries: connection refused
```

1. Verify S3 endpoint is reachable:
   ```bash
   curl -I http://minio:9000
   ```
2. Check credentials:
   ```bash
   mc alias set test http://minio:9000 $S3_ACCESS_KEY $S3_SECRET_KEY
   mc ls test/
   ```
3. Check dead-letter for failed files:
   ```bash
   ls -la /var/lib/hoard/dead-letter/
   ```

### High pending count (never drains)

Usually caused by WAL mode in `pending.db`. Force checkpoint:

```bash
sqlite3 /var/lib/hoard/pending.db "PRAGMA wal_checkpoint(TRUNCATE);"
```

Then trigger a flush:

```bash
hoard ctl flush default
```

### ETag mismatches

{: .note }
Fixed in v0.6.0+. Hoard now uses `pread(2)` on an independent fd to
compute MD5 before `sendfile`, eliminating the TOCTOU race that caused
ETag mismatches during concurrent writes.

If you still see ETag mismatches on v0.6+, use the `hoard-atomic` wrapper
for overwrite-heavy workloads:

```bash
cat payload.json | hoard-atomic /var/lib/hoard/volumes/app/data.json
```
