---
sidebar_position: 4
---

# Operations

## Restore files

### List S3 contents

```bash
# Via MinIO client (mc)
mc ls local/hoard-backups/hoard/
mc ls -r local/hoard-backups/hoard/

# Via control socket
echo '{"list":"/var/lib/hoard/volumes"}' | nc -U /var/run/hoard.sock | jq .
```

### Restore single file

```bash
mc cp local/hoard-backups/hoard/path/to/file.txt ./restored.txt
```

### Bulk restore

```bash
mc cp -r local/hoard-backups/hoard/ ./restore-root/
# OR via control socket for specific volume
echo '{"restore":"/var/lib/hoard/volumes/postgres/schema/v2.sql"}' \
  | nc -U /var/run/hoard.sock | jq .
```

## Metrics

Endpoint: `http://0.0.0.0:9150/metrics`

| Metric | Type | Description |
|--------|------|-------------|
| `hoard_uploads_total` | Counter | Total uploads (success + retry) |
| `hoard_upload_errors_total` | Counter | Failed uploads |
| `hoard_upload_bytes_total` | Counter | Total bytes uploaded |
| `hoard_upload_duration_seconds` | Histogram | Upload latency distribution |
| `hoard_pending_files` | Gauge | Files in pending queue |
| `hoard_dead_letter_total` | Gauge | Files in dead-letter directory |
| `hoard_bpf_events_total` | Counter | BPF events received |
| `hoard_pending_db_size` | Gauge | SQLite pending database size |

### Alert rules (Prometheus)

```yaml
groups:
  - name: hoard
    rules:
      - alert: HoardUploadErrorRateHigh
        expr: rate(hoard_upload_errors_total[5m]) / rate(hoard_uploads_total[5m]) > 0.05
        for: 10m
        annotations:
          summary: "Upload error rate above 5%"

      - alert: HoardPendingQueueGrowing
        expr: hoard_pending_files > 1000
        for: 15m
        annotations:
          summary: "Pending queue over 1000 files"

      - alert: HoardDeadLetterQueueGrowing
        expr: hoard_dead_letter_total > 10
        for: 5m
        annotations:
          summary: "Dead-letter queue has 10+ files"

      - alert: HoardBPFNotReceiving
        expr: rate(hoard_bpf_events_total[5m]) == 0
        for: 10m
        annotations:
          summary: "No BPF events in last 5 minutes"

      - alert: HoardUploadsFailing
        expr: increase(hoard_uploads_total[5m]) == 0
        for: 10m
        annotations:
          summary: "No successful uploads in last 5 minutes"
```

## Garbage collection

```bash
# Trigger manually via control socket
echo '{"gc":{}}' | nc -U /var/run/hoard.sock

# Response: {"gc": {"deleted": 15, "errors": 0}}
```

GC runs on a schedule (default every 6 hours). Removes S3 objects older
than `ttl_days` (default 30). Dry-run mode available via `--gc-dry-run`.

## Dead-letter queue

Files that fail upload after `max_upload_retries` (default 5) land in the
dead-letter directory (default `/var/lib/hoard/dead-letter`).

```bash
# List dead-letter files
ls /var/lib/hoard/dead-letter/

# Reprocess a dead-letter file
echo '{"reprocess":"dead-letter.txt"}' | nc -U /var/run/hoard.sock

# Bulk reprocess
echo '{"reprocess_all":{}}' | nc -U /var/run/hoard.sock
```

## Troubleshooting

### Hoard not detecting file writes

1. Check BPF program loads: `hoard --check-bpf`
2. Verify kernel ≥ 5.5: `uname -r`
3. Check BTF available: `ls /sys/kernel/btf/vmlinux`
4. Increase log verbosity: `RUST_LOG=debug hoard`
5. Verify watch path has `inotify` + BPF perms

### Upload failing

1. Check S3 connectivity: `curl -I $HOARD_S3_ENDPOINT`
2. Verify bucket exists: `mc ls local/`
3. Check credentials: `mc admin info local`
4. Increase `RUST_LOG=debug` for SigV4 signing details
5. Try `no_sign = true` for MinIO dev mode

### Pending queue growing

1. Check S3 upload latency: `hoard_upload_duration_seconds`
2. Increase drain frequency via config
3. Check network between node and S3
4. Verify S3 rate limits not hit

### BPF events not arriving

```bash
# Debug BPF hook status
hoard --debug-bpf
# Output: hooks=2 loaded=2 buffer=ringbuffer capacity=262144

# Check BPF system requirements
bpftool prog list | grep -A5 hoard
```
