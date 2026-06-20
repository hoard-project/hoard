---
title: CLI Reference
nav_order: 7
parent: Reference
---

# CLI Reference

`hoard` is the main binary. Subcommands: `daemon`, `ctl`, `restore`.

---

## `hoard daemon`

Start the Hoard daemon.

```bash
hoard daemon [OPTIONS]
```

### Flags

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--config <PATH>` | path | auto-detect | TOML config file path |
| `--mode <MODE>` | `standalone` or `nomad` | `standalone` | Runtime mode |
| `--watch-root <PATH>` | path | — | Root directory to watch |
| `--s3-endpoint <URL>` | URL | — | S3 endpoint |
| `--s3-bucket <NAME>` | string | — | S3 bucket name |
| `--s3-access-key <KEY>` | string | — | S3 access key |
| `--s3-secret-key <KEY>` | string | — | S3 secret key |
| `--s3-prefix <PREFIX>` | string | `""` | S3 key prefix |
| `--s3-region <REGION>` | string | `us-east-1` | AWS region |
| `--s3-no-sign` | flag | `false` | Skip SigV4 signing |
| `--filter-extensions <EXTS>` | comma-separated | `*` | File extensions |
| `--gc-interval-secs <N>` | u64 | `21600` | GC interval |
| `--gc-ttl-days <N>` | u64 | `30` | Object TTL |
| `--metrics-addr <ADDR>` | host:port | `0.0.0.0:9150` | Metrics endpoint |
| `--nomad-addr <URL>` | URL | — | Nomad API (nomad mode) |
| `--nomad-token <TOKEN>` | string | — | Nomad ACL token |

### Env vars

Every flag maps to an env var (see [Configuration]({{ site.baseurl }}/configuration#environment-variable-mapping-v1)).
Flag > env var > TOML.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Clean shutdown (SIGTERM drained) |
| `1` | Configuration error |
| `2` | BPF load failure (falls back to periodic scan) |
| `3` | S3 connection failure |

---

## `hoard ctl`

Control a running Hoard daemon via Unix socket.

### `hoard ctl status <SERVICE>`

Query daemon status.

```bash
hoard ctl status default
```

Output:

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

| Field | Type | Description |
|-------|------|-------------|
| `service` | string | Logical service name |
| `mode` | string | `"standalone"` or `"nomad"` |
| `health` | string | `"ok"` or `"degraded"` |
| `pending_files` | u64 | Files awaiting next drain |
| `total_uploads` | u64 | Total uploads since start |
| `dead_letter_files` | u64 | Files in dead-letter directory |
| `uptime_secs` | u64 | Seconds since daemon start |

### `hoard ctl flush <SERVICE>`

Trigger immediate drain of pending set.

```bash
hoard ctl flush default
# Flush complete: 3 files uploaded, 0 failed
```

Blocks until drain completes. Exit code `0` = all files uploaded, `1` = some failed.

### `hoard ctl restore <SERVICE> [OPTIONS]`

Restore files from S3. See [Operations]({{ site.baseurl }}/operations#restore).

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--target <PATH>` | path | — | Restore destination directory (required) |
| `--prefix <PREFIX>` | string | `""` | S3 prefix filter |
| `--list` | flag | false | List objects instead of restoring |
| `--dry-run` | flag | false | Show what would be restored |

---

## `hoard-atomic`

Standalone helper binary for atomic file writes.

```bash
cat data.json | hoard-atomic /var/lib/hoard/volumes/app/data.json
```

Reads stdin → temp file → `fsync` → atomic rename. Prevents Hoard from
seeing half-written files during overwrite-heavy workloads.

| Argument | Required | Description |
|----------|----------|-------------|
| `<TARGET>` | yes | Destination path for the atomic write |

### Behavior

1. Creates temp file in target's parent directory
2. Copies stdin to temp file (kernel-buffered)
3. `fsync` temp file
4. `rename` temp → target (atomic on same filesystem)
5. `fsync` parent directory (best-effort)

Exit code `0` on success, `1` on error.
