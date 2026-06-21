# CLI reference

```text
hoard [OPTIONS] [COMMAND]
```

All flags can also be set via environment variables (see [env-vars](env-vars.md))
or TOML config (see [configuration](../configuration.md)).

Priority: CLI flag > env var > TOML value > default.

## Flags

| Flag | Type | Default | Env | Description |
|------|------|---------|-----|-------------|
| `--config <PATH>` | path | auto-discovery | `HOARD_CONFIG` | TOML config file |
| `--mode <MODE>` | `standalone` \| `nomad` | `standalone` | `HOARD_MODE` | Runtime mode |
| `--service <NAME>` | string | `"default"` | `HOARD_SERVICE` | Logical service name |
| `--watch-path <PATH>` | path | `/var/lib/hoard/volumes` | `HOARD_WATCH_PATH` | Watch directory |
| `--watch-root <PATH>` | path | — | `HOARD_WATCH_ROOT` | Root for Nomad volumes (overrides) |
| `--watch-patterns <P>` | comma-sep | `*` | `HOARD_WATCH_PATTERNS` | File glob patterns |
| `--watch-excludes <E>` | comma-sep | `*.tmp,*.journal` | `HOARD_WATCH_EXCLUDES` | Patterns to exclude |
| `--tls-mode <M>` | `ktls` \| `plain` \| `userspace` | `plain` | `HOARD_TLS_MODE` | TLS mode |
| `--s3-endpoint <URL>` | URL | `http://localhost:9000` | `HOARD_S3_ENDPOINT` | S3 endpoint |
| `--s3-region <NAME>` | string | `us-east-1` | `HOARD_S3_REGION` | S3 region |
| `--s3-bucket <NAME>` | string | `backups` | `HOARD_S3_BUCKET` | S3 bucket |
| `--s3-access-key <KEY>` | string | `""` | `HOARD_S3_ACCESS_KEY` | S3 access key |
| `--s3-secret-key <KEY>` | string | `""` | `HOARD_S3_SECRET_KEY` | S3 secret key |
| `--s3-no-sign` | flag | `false` | `HOARD_S3_NO_SIGN` | Disable SigV4 signing |
| `--s3-prefix <P>` | string | `"default"` | `HOARD_S3_PREFIX` | S3 key prefix |
| `--gc-interval <SECS>` | u64 | `3600` (1h) | `HOARD_GC_INTERVAL` | GC interval (seconds) |
| `--gc-ttl-days <DAYS>` | u32 | `30` | `HOARD_GC_TTL_DAYS` | GC TTL (days) |
| `--pending-db <PATH>` | path | `/var/lib/hoard/pending.db` | `HOARD_PENDING_DB` | Pending-set DB |
| `--max-upload-retries <N>` | u32 | `5` | `HOARD_MAX_UPLOAD_RETRIES` | Max upload retries |
| `--dead-letter-dir <PATH>` | path | `/var/lib/hoard/dead-letter` | `HOARD_DEAD_LETTER_DIR` | Dead-letter dir |
| `--nomad-addr <URL>` | URL | `""` | `HOARD_NOMAD_ADDR` | Nomad API address |
| `--nomad-token <T>` | string | `""` | `HOARD_NOMAD_TOKEN` | Nomad ACL token |
| `--nomad-meta-enabled` | flag | `false` | `HOARD_NOMAD_META_ENABLED` | Enable meta discovery |
| `--nomad-meta-poll-secs <N>` | u64 | `300` | `HOARD_NOMAD_META_POLL_SECS` | Meta poll interval |
| `--control-socket <PATH>` | path | `/var/run/hoard.sock` | `HOARD_CONTROL_SOCKET` | Control socket path |
| `--metrics-addr <ADDR>` | socket addr | `0.0.0.0:9150` | `HOARD_METRICS_ADDR` | Metrics endpoint |

Subcommands also accept these flags via environment variables to override config/TTL sources.

## Subcommands

### `hoard nomad-restore`

Pull backups from S3 into a Nomad alloc directory. Designed as a **prestart hook**.

```text
hoard nomad-restore [OPTIONS]
```

All S3 flags auto-detected from environment (`HOARD_S3_*` or `NOMAD_META_hoard_*`)
when running inside a Nomad task. Explicit flags override auto-detection.

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--s3-endpoint <URL>` | URL | auto | S3 endpoint |
| `--s3-region <NAME>` | string | `us-east-1` | S3 region |
| `--s3-bucket <NAME>` | string | auto | S3 bucket |
| `--s3-access-key <KEY>` | string | auto | S3 access key |
| `--s3-secret-key <KEY>` | string | auto | S3 secret key |
| `--prefix <P>` | string | auto | S3 prefix (auto: `NOMAD_META_hoard_prefix`) |
| `--dest <PATH>` | path | auto | Destination (auto: `NOMAD_ALLOC_DIR`) |
| `--if-empty` | flag | `false` | Skip restore if dest dir non-empty |
| `--dry-run` | flag | `false` | List files, don't download |
| `--force` | flag | `false` | Overwrite existing files |

### `hoard restore`

Bulk restore from S3 (standalone, no Nomad auto-detection).

```text
hoard restore [OPTIONS]
```

### `hoard ctl`

Control the running daemon via Unix socket.

```text
hoard ctl flush <SERVICE>
hoard ctl status <SERVICE>
```

## Examples

```bash
# Run with TOML config
hoard --config /etc/hoard/hoard.toml

# Override values via CLI
hoard --config /etc/hoard/hoard.toml \
  --s3-bucket production-backups \
  --watch-patterns db,sqlite

# Nomad restore (prestart hook)
hoard nomad-restore --if-empty

# Manual restore with explicit S3
hoard nomad-restore \
  --dest /tmp/restore \
  --s3-endpoint http://s3:9000 \
  --s3-bucket backups \
  --prefix databases/postgres

# Trigger immediate drain via control socket
hoard ctl flush default
```

## Signals

| Signal | Action |
|--------|--------|
| `SIGTERM` / `SIGINT` | Graceful shutdown (drain pending queue) |
| `SIGUSR1` | Trigger immediate pending drain |

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Clean exit |
| 1 | Config error |
| 2 | BPF load failure |
| 3 | S3 connectivity failure |
| 4 | Runtime error |
