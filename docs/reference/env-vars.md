---
title: Environment Variables
nav_order: 9
parent: Reference
---

# Environment Variables

Every config key can be set via environment variable. This is the complete
mapping.

---

## Daemon

| Variable | Type | Maps to | Default |
|----------|------|---------|---------|
| `HOARD_CONFIG` | path | config file path | auto-detect |
| `HOARD_MODE` | `standalone` or `nomad` | `daemon.mode` | `standalone` |
| `HOARD_SERVICE` | string | `daemon.service` | `default` |
| `HOARD_CONTROL_SOCKET` | path | `daemon.control_socket` | `/run/hoard/<service>.sock` |
| `HOARD_METRICS_ADDR` | host:port | `daemon.metrics_addr` | `0.0.0.0:9150` |

## Watch

| Variable | Type | Maps to | Default |
|----------|------|---------|---------|
| `HOARD_WATCH_ROOT` | path | `watch.path` | — |

## S3

| Variable | Type | Maps to | Default |
|----------|------|---------|---------|
| `HOARD_S3_ENDPOINT` | URL | `s3.endpoint` | — |
| `HOARD_S3_BUCKET` | string | `s3.bucket` | — |
| `HOARD_S3_REGION` | string | `s3.region` | `us-east-1` |
| `HOARD_S3_ACCESS_KEY` | string | `s3.access_key` | — |
| `HOARD_S3_SECRET_KEY` | string | `s3.secret_key` | — |
| `HOARD_S3_PREFIX` | string | `s3.prefix` | `""` |
| `HOARD_S3_NO_SIGN` | `true` or `false` | `s3.no_sign` | `false` |

## GC

| Variable | Type | Maps to | Default |
|----------|------|---------|---------|
| `HOARD_GC_INTERVAL_SECS` | u64 | `gc.interval_secs` | `21600` |
| `HOARD_GC_TTL_DAYS` | u64 | `gc.ttl_days` | `30` |

## Filter

| Variable | Type | Maps to | Default |
|----------|------|---------|---------|
| `HOARD_FILTER_EXTENSIONS` | comma-separated | `filter.extensions` | `*` |
| `HOARD_FILTER_EXCLUDE` | comma-separated | `filter.exclude` | `""` |

## Resilience

| Variable | Type | Maps to | Default |
|----------|------|---------|---------|
| `HOARD_PENDING_DB` | path | `resilience.pending_db` | `/var/lib/hoard/pending.db` |
| `HOARD_MAX_UPLOAD_RETRIES` | u32 | `resilience.max_upload_retries` | `5` |
| `HOARD_DEAD_LETTER_DIR` | path | `resilience.dead_letter_dir` | `/var/lib/hoard/dead-letter` |

## Nomad

| Variable | Type | Maps to | Default |
|----------|------|---------|---------|
| `HOARD_NOMAD_ADDR` | URL | `nomad.addr` | `http://127.0.0.1:4646` |
| `HOARD_NOMAD_TOKEN` | string | `nomad.token` | — |

---

## Priority

```
CLI flag  >  env var  >  TOML  >  default
```

Example: if both `--s3-bucket prod-backups` and `HOARD_S3_BUCKET=dev-backups`
are set, the CLI flag wins.

---

## TOML `${ENV_VAR}` expansion

Inside TOML config files, use `${ENV_VAR}` syntax:

```toml
[s3]
access_key = "${S3_ACCESS_KEY}"
secret_key = "${S3_SECRET_KEY}"
```

This is resolved at startup. Unknown variables are left as-is (no error).
