# Environment variables

!!! tip "Priority"
    CLI flag > env var > TOML value > default

## All variables

| Variable | Type | Default | Maps to |
|----------|------|---------|---------|
| `HOARD_CONFIG` | path | auto-discovery | `--config` |
| `HOARD_MODE` | `standalone` \| `nomad` | `standalone` | `daemon.mode` |
| `HOARD_WATCH_ROOT` | path | — | `watch.path` |
| `HOARD_S3_ENDPOINT` | URL | — | `s3.endpoint` |
| `HOARD_S3_BUCKET` | string | — | `s3.bucket` |
| `HOARD_S3_REGION` | string | `us-east-1` | `s3.region` |
| `HOARD_S3_ACCESS_KEY` | string | — | `s3.access_key` |
| `HOARD_S3_SECRET_KEY` | string | — | `s3.secret_key` |
| `HOARD_S3_PREFIX` | string | `""` | `s3.prefix` |
| `HOARD_S3_NO_SIGN` | `true` \| `false` | `false` | `s3.no_sign` |
| `HOARD_DEBOUNCE_MS` | u64 | `100` | `watch.debounce_ms` |
| `HOARD_DRAIN_INTERVAL` | u64 | `30` | `daemon.drain_interval_secs` |
| `HOARD_METRICS_ADDR` | socket addr | `0.0.0.0:9150` | `daemon.metrics_addr` |
| `HOARD_GC_INTERVAL_SECS` | u64 | `21600` | `gc.interval_secs` |
| `HOARD_GC_TTL_DAYS` | u64 | `30` | `gc.ttl_days` |
| `HOARD_FILTER_EXTENSIONS` | comma-separated | `*` | `filter.extensions` |
| `HOARD_FILTER_EXCLUDE` | comma-separated | `""` | `filter.exclude` |
| `HOARD_PENDING_DB` | path | `/var/lib/hoard/pending.db` | `resilience.pending_db` |
| `HOARD_MAX_UPLOAD_RETRIES` | u32 | `5` | `resilience.max_upload_retries` |
| `HOARD_DEAD_LETTER_DIR` | path | `/var/lib/hoard/dead-letter` | `resilience.dead_letter_dir` |
| `HOARD_NODE_HOST` | string | `hostname` | Nomad cluster identifier |
| `HOARD_SERVICE` | string | `default` | `daemon.service` |
| `RUST_LOG` | log level | `info` | Log verbosity |

## Examples

```bash
# Minimal production
HOARD_MODE=standalone \
HOARD_WATCH_ROOT=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=http://s3:9000 \
HOARD_S3_BUCKET=backups \
HOARD_S3_ACCESS_KEY=s3admin \
HOARD_S3_SECRET_KEY=s3admin123 \
  hoard

# Full production with all knobs
HOARD_MODE=standalone \
HOARD_WATCH_ROOT=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=https://s3.us-east-1.amazonaws.com \
HOARD_S3_BUCKET=prod-backups \
HOARD_S3_ACCESS_KEY=AKIAIOSFODNN7EXAMPLE \
HOARD_S3_SECRET_KEY=wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY \
HOARD_DEBOUNCE_MS=200 \
HOARD_DRAIN_INTERVAL=60 \
HOARD_GC_TTL_DAYS=90 \
HOARD_FILTER_EXTENSIONS=db,sqlite,log,json,csv,parquet \
HOARD_FILTER_EXCLUDE=*.tmp,*.lock \
HOARD_METRICS_ADDR=0.0.0.0:9090 \
HOARD_MAX_UPLOAD_RETRIES=3 \
RUST_LOG=warn,hoard=info \
  hoard

# Debug mode
RUST_LOG=debug hoard --check-bpf
```
