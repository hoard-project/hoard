# Operations

## Restore files

```bash
# List S3 contents via mc CLI
mc ls local/hoard-backups/hoard/
mc ls -r local/hoard-backups/hoard/

# Restore single file
mc cp local/hoard-backups/hoard/path/to/file.txt ./restored.txt

# Bulk restore
mc cp -r local/hoard-backups/hoard/ ./restore-root/
```

## Metrics

Endpoint: `http://0.0.0.0:9150/metrics` (Prometheus OpenMetrics format)

| Metric | Type | Description |
|--------|------|-------------|
| `hoard_upload_total` | Counter | Total uploads attempted |
| `hoard_upload_bytes_total` | Counter | Total bytes uploaded |
| `hoard_upload_in_flight` | Gauge | Uploads currently in progress |
| `hoard_upload_duration_seconds` | Histogram | Upload latency distribution |
| `hoard_upload_failures_total` | Counter | Failed uploads |
| `hoard_etag_mismatch_total` | Counter | ETag verification failures |
| `hoard_pending_files` | Gauge | Files in pending queue |
| `hoard_dead_letter_files` | Gauge | Files in dead-letter directory |
| `hoard_ringbuf_events_total` | Counter | BPF ring buffer events received |
| `hoard_gc_cycles_total` | Counter | GC cycles completed |
| `hoard_gc_deleted_total` | Counter | Objects deleted by GC |
| `hoard_gc_errors_total` | Counter | GC errors |
| `hoard_health_status` | Gauge | 1=ok, 0=degraded |

### Health endpoint

```bash
curl http://127.0.0.1:9150/health
# {"status":"ok","pending":0.0,"dead_letter":0.0}
```

Returns `"ok"` when healthy (BPF loaded, S3 reachable, dead-letter empty).
Returns `"degraded"` when S3 unreachable or dead-letter files present.

### Alert rules

```yaml
groups:
  - name: hoard
    rules:
      - alert: HoardUploadErrorRateHigh
        expr: rate(hoard_upload_failures_total[5m]) / rate(hoard_upload_total[5m]) > 0.05
        for: 10m
        annotations:
          summary: "Upload error rate above 5%"

      - alert: HoardPendingQueueGrowing
        expr: hoard_pending_files > 1000
        for: 15m
        annotations:
          summary: "Pending queue over 1000 files"

      - alert: HoardDeadLetterQueueGrowing
        expr: hoard_dead_letter_files > 10
        for: 5m
        annotations:
          summary: "Dead-letter queue has 10+ files"

      - alert: HoardBPFNotReceiving
        expr: rate(hoard_ringbuf_events_total[5m]) == 0
        for: 10m
        annotations:
          summary: "No BPF events in last 5 minutes"

      - alert: HoardHealthDegraded
        expr: hoard_health_status < 1
        for: 2m
        annotations:
          summary: "Hoard daemon is degraded"
```

## Garbage collection

GC runs on a schedule (default every hour). Removes S3 objects older
than `ttl_days` (default 30).

```bash
# Trigger via control socket (standalone mode)
echo flush | nc -U /var/run/hoard.sock

# Trigger via HTTP (metrics server)
curl -X POST http://127.0.0.1:9150/flush
```

## Dead-letter queue

Files that fail upload after `max_upload_retries` (default 5) land in the
dead-letter directory (default `/var/lib/hoard/dead-letter`).

```bash
# List dead-letter files
ls /var/lib/hoard/dead-letter/

# Manually re-upload a dead-letter file
mc cp /var/lib/hoard/dead-letter/bad-file.txt local/hoard-backups/hoard/
```

## Troubleshooting

### Hoard not detecting file writes

1. Verify BPF loaded: `curl -s http://127.0.0.1:9150/health | grep "ok"`
2. If degraded, check logs for "BPF load" errors
3. Verify kernel ≥ 5.5: `uname -r`
4. Check BTF available: `ls /sys/kernel/btf/vmlinux`
5. Increase log verbosity: `RUST_LOG=debug hoard`
6. Verify watch path exists and is writable

### Upload failing

1. Check S3 connectivity: `curl -I $HOARD_S3_ENDPOINT`
2. Verify bucket exists: `mc ls local/`
3. Check credentials: `mc admin info local`
4. Increase `RUST_LOG=debug` for SigV4 signing details
5. Try `no_sign = true` for local S3 (MinIO) dev mode

### Pending queue growing

1. Check S3 upload latency: `hoard_upload_duration_seconds`
2. Trigger manual flush: `curl -X POST http://127.0.0.1:9150/flush`
3. Check network between node and S3
4. Verify S3 rate limits not hit

### BPF events not arriving

```bash
# Check BPF system requirements
bpftool prog list | grep -A5 hoard
```
