# CLI reference

```text
hoard [OPTIONS]
```

## Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--config <PATH>` | path | auto-discovery | TOML config file |
| `--mode <MODE>` | `standalone` \| `nomad` | `standalone` | Runtime mode |
| `--service <NAME>` | string | `default` | Logical service name |
| `--watch-path <PATH>` | path | config | Watch directory |
| `--watch-patterns <P>` | comma-sep | `*` | File glob patterns |
| `--watch-excludes <E>` | comma-sep | — | Patterns to exclude |
| `--s3-endpoint <URL>` | URL | config | S3 endpoint |
| `--s3-region <NAME>` | string | config | S3 region |
| `--s3-bucket <NAME>` | string | config | S3 bucket |
| `--s3-access-key <KEY>` | string | config | S3 access key |
| `--s3-secret-key <KEY>` | string | config | S3 secret key |
| `--s3-no-sign` | flag | `false` | Disable SigV4 signing |
| `--s3-prefix <P>` | string | config | S3 key prefix |
| `--gc-interval <SECS>` | u64 | `21600` | GC interval (seconds) |
| `--gc-ttl-days <DAYS>` | u32 | `30` | GC TTL (days) |
| `--pending-db <PATH>` | path | `/var/lib/hoard/pending.db` | Pending-set DB |
| `--max-upload-retries <N>` | u32 | `5` | Max upload retries |
| `--dead-letter-dir <PATH>` | path | `/var/lib/hoard/dead-letter` | Dead-letter dir |
| `--nomad-addr <URL>` | URL | `http://127.0.0.1:4646` | Nomad API address |
| `--nomad-token <T>` | string | — | Nomad ACL token |
| `--nomad-meta-enabled` | flag | `false` | Enable meta discovery |
| `--nomad-meta-poll-secs <N>` | u64 | `30` | Meta poll interval |
| `--metrics-addr <ADDR>` | socket addr | `0.0.0.0:9150` | Metrics endpoint |
| `--tls-mode <M>` | `plain` \| `tls` \| `ktls` | `plain` | TLS mode |
| `--check-bpf` | flag | — | Load BPF, print status, exit |
| `--debug-bpf` | flag | — | Print BPF details, exit |
| `--gc-dry-run` | flag | — | GC without deleting |
| `--dump-config` | flag | — | Print resolved config, exit |
| `--version` | flag | — | Print version, exit |

## Subcommands

### `hoard nomad-restore`

Restore backups into a Nomad alloc directory. Intended as a Nomad `prestart` hook.

```text
hoard nomad-restore [OPTIONS] --alloc-dir <PATH>
```

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--alloc-dir <PATH>` | path | — | Nomad alloc directory (**required**) |
| `--watch-root <PATH>` | path | config | Base watch directory |
| `--s3-endpoint <URL>` | URL | config | S3 endpoint |
| `--s3-bucket <NAME>` | string | config | S3 bucket |
| `--s3-access-key <KEY>` | string | config | S3 access key |
| `--s3-secret-key <KEY>` | string | config | S3 secret key |
| `--no-sign` | flag | `false` | Skip SigV4 |
| `--if-empty` | flag | `false` | Only restore if dir is empty |
| `--dry-run` | flag | `false` | List files, don't download |
| `--force` | flag | `false` | Overwrite existing files |

## Examples

```bash
# Run with TOML config
hoard --config /etc/hoard/hoard.toml

# Override values via CLI
hoard --config /etc/hoard/hoard.toml \
  --s3-bucket production-backups \
  --watch-patterns db,sqlite

# Check BPF program loads
hoard --check-bpf
# Output: hooks=2 loaded=2 buffer=ringbuffer capacity=262144

# Dry-run garbage collection
hoard --gc-dry-run
# Output: would delete 15 objects (45.2 MB)

# Nomad restore (prestart hook)
hoard nomad-restore \
  --alloc-dir /opt/nomad/data/alloc/abc123/alloc \
  --if-empty \
  --s3-endpoint http://s3:9000 \
  --s3-bucket backups
```

## Signals

| Signal | Action |
|--------|--------|
| `SIGTERM` / `SIGINT` | Graceful shutdown (drain pending) |
| `SIGUSR1` | Trigger immediate pending drain |

## Exit codes

| Code | Meaning |
|------|---------|
| 0 | Clean exit |
| 1 | Config error |
| 2 | BPF load failure |
| 3 | S3 connectivity failure |
| 4 | Runtime error |
