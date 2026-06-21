# Environment variables

!!! tip "Priority"
    CLI flag > env var > TOML value > default

## Core

| Variable | Type | Default | Maps to / notes |
|----------|------|---------|-----------------|
| `HOARD_CONFIG` | path | auto-discovery | `--config` |
| `HOARD_MODE` | `standalone` \| `nomad` | `standalone` | `daemon.mode` |

## Watch

| Variable | Type | Default | Maps to / notes |
|----------|------|---------|-----------------|
| `HOARD_WATCH_PATH` | path | `/var/lib/hoard/volumes` | `watch.paths[0]` |
| `HOARD_WATCH_PATTERNS` | comma-separated | `*` | `defaults.extensions` |
| `HOARD_WATCH_EXCLUDES` | comma-separated | `*.tmp,*.journal` | `defaults.exclude` |

## S3

| Variable | Type | Default | Maps to / notes |
|----------|------|---------|-----------------|
| `HOARD_S3_ENDPOINT` | URL | `http://localhost:9000` | `s3.endpoint` |
| `HOARD_S3_BUCKET` | string | `backups` | `s3.bucket` |
| `HOARD_S3_REGION` | string | `us-east-1` | `s3.region` |
| `HOARD_S3_ACCESS_KEY` | string | `""` | `s3.access_key` |
| `HOARD_S3_SECRET_KEY` | string | `""` | `s3.secret_key` |
| `HOARD_S3_NO_SIGN` | `true` \| `false` | `false` | `s3.no_sign` |
| `HOARD_S3_PREFIX` | string | `"default"` | `defaults.prefix` |

## Daemon

| Variable | Type | Default | Maps to / notes |
|----------|------|---------|-----------------|
| `HOARD_METRICS_ADDR` | socket addr | `0.0.0.0:9150` | `daemon.metrics_addr` |
| `HOARD_GC_INTERVAL` | u64 | `3600` (1h) | `--gc-interval` — no TOML section |
| `HOARD_GC_TTL_DAYS` | u32 | `30` | `--gc-ttl-days` — no TOML section |
| `HOARD_CONTROL_SOCKET` | path | `/var/run/hoard.sock` | `daemon.control_socket` |
| `HOARD_SERVICE` | string | `"default"` | `daemon.service` |
| `HOARD_TLS_MODE` | `plain` \| `tls` | `plain` | `--tls-mode` |

## Resilience

| Variable | Type | Default | Maps to / notes |
|----------|------|---------|-----------------|
| `HOARD_PENDING_DB` | path | `/var/lib/hoard/pending.db` | `resilience.pending_db` |
| `HOARD_MAX_UPLOAD_RETRIES` | u32 | `5` | `resilience.max_upload_retries` |
| `HOARD_DEAD_LETTER_DIR` | path | `/var/lib/hoard/dead-letter` | `resilience.dead_letter_dir` |

## Nomad

| Variable | Type | Default | Maps to / notes |
|----------|------|---------|-----------------|
| `HOARD_NOMAD_ADDR` | URL | `""` | `nomad.addr` |
| `HOARD_NOMAD_TOKEN` | string | `""` | `nomad.token` |
| `HOARD_NOMAD_META_ENABLED` | `true` \| `false` | `false` | `nomad.meta_enabled` |
| `HOARD_NOMAD_META_POLL_SECS` | u64 | `300` | `nomad.meta_poll_secs` |

## Logging

| Variable | Type | Default | Maps to / notes |
|----------|------|---------|-----------------|
| `RUST_LOG` | log level | `info` | Log verbosity |

## Examples

```bash
# Minimal production
HOARD_MODE=standalone \
HOARD_WATCH_PATH=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=http://s3:9000 \
HOARD_S3_BUCKET=backups \
HOARD_S3_ACCESS_KEY=s3admin \
HOARD_S3_SECRET_KEY=s3admin123 \
  hoard

# Full production
HOARD_MODE=standalone \
HOARD_WATCH_PATH=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=https://s3.us-east-1.amazonaws.com \
HOARD_S3_BUCKET=prod-backups \
HOARD_S3_ACCESS_KEY=AKIAIOSFODNN7EXAMPLE \
HOARD_S3_SECRET_KEY=wJalrXUtnFEMI/K7MDENG \
HOARD_WATCH_PATTERNS=db,sqlite,log,json,csv,parquet \
HOARD_WATCH_EXCLUDES=*.tmp,*.lock \
HOARD_METRICS_ADDR=0.0.0.0:9090 \
HOARD_GC_TTL_DAYS=90 \
HOARD_MAX_UPLOAD_RETRIES=3 \
RUST_LOG=warn,hoard=info \
  hoard

# Nomad mode with meta auto-discovery
HOARD_MODE=nomad \
HOARD_WATCH_PATH=/opt/nomad/volumes \
HOARD_NOMAD_ADDR=http://127.0.0.1:4646 \
HOARD_NOMAD_TOKEN=54285c4f-... \
HOARD_NOMAD_META_ENABLED=true \
HOARD_NOMAD_META_POLL_SECS=300 \
HOARD_S3_ENDPOINT=http://s3:9000 \
HOARD_S3_BUCKET=backups \
HOARD_S3_ACCESS_KEY=s3admin \
HOARD_S3_SECRET_KEY=s3admin123 \
  hoard

# Debug mode
RUST_LOG=debug hoard
```
