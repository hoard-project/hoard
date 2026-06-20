# CLI reference

```
hoard [OPTIONS]
```

## Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--config <PATH>` | path | auto-discovery | TOML config file |
| `--mode <MODE>` | `standalone` \| `nomad` | `standalone` | Runtime mode |
| `--version` | — | — | Print version and exit |
| `--check-bpf` | — | — | Load BPF, print hook status, exit |
| `--debug-bpf` | — | — | Print BPF program details, exit |
| `--gc-dry-run` | — | — | Run GC without deleting |
| `--dump-config` | — | — | Print resolved config, exit |
| `--watch-root <PATH>` | path | config | Override watch root |
| `--s3-endpoint <URL>` | URL | config | Override S3 endpoint |
| `--s3-bucket <NAME>` | string | config | Override S3 bucket |
| `--s3-access-key <KEY>` | string | config | Override S3 access key |
| `--s3-secret-key <KEY>` | string | config | Override S3 secret key |
| `--no-sign` | — | `false` | Disable S3 SigV4 signing |
| `--debounce-ms <MS>` | u64 | `100` | Debounce window (ms) |
| `--drain-interval <SECS>` | u64 | `30` | Drain interval (seconds) |
| `--metrics-addr <ADDR>` | socket addr | `0.0.0.0:9150` | Prometheus endpoint |

## Examples

```bash
# Print version
hoard --version

# Run with TOML config
hoard --config /etc/hoard/hoard.toml

# Override specific values via CLI
hoard --config /etc/hoard/hoard.toml \
  --s3-bucket production-backups \
  --debounce-ms 200

# Check BPF program loads
hoard --check-bpf
# Output:
# hooks=2 loaded=2 buffer=ringbuffer capacity=262144

# Dry-run garbage collection
hoard --gc-dry-run
# Output:
# gc dry-run: would delete 15 objects (45.2 MB), 0 errors
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
| 4 | Runtime panic / unexpected error |
