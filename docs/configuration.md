---
title: Configuration
nav_order: 3
---

# Configuration

Hoard supports two config formats: **v1** (flat TOML) and **v2**
(StorageClass + Volume model). The v2 format is recommended for
production.

{: .important }
All string values support `${ENV_VAR}` expansion. Priority:
CLI flag > env var > TOML value > default.

---

## Quick config (v1)

Minimal working config. Good for single-volume setups.

```toml
[daemon]
mode = "standalone"

[watch]
path = "/var/lib/hoard/volumes"

[s3]
endpoint   = "http://127.0.0.1:9000"
bucket     = "my-backups"
access_key = "${S3_ACCESS_KEY}"
secret_key = "${S3_SECRET_KEY}"

[gc]
interval_secs = 21600
ttl_days      = 30

[filter]
extensions = ["db", "sqlite", "sqlite3", "log", "json", "csv"]
```

---

## v2: StorageClass + Volume model

{: .note }
v2 config uses `conf.d/` directory hot-reload. Place `.toml` files in
`/etc/hoard/conf.d/` — Hoard watches this directory and reloads on change.

### Concepts

| Concept | Analogous to | Purpose |
|---------|-------------|---------|
| **StorageClass** | K8s StorageClass | Reusable policy template (TTL, retries, compression) |
| **Volume** | K8s PVC | Path glob → StorageClass + S3 prefix binding |
| **Default** | StorageClass fallback | Applied when no volume matches |

### Full v2 example

```toml
[hoard]
version = 2

[daemon]
mode = "standalone"
service = "production"

[watch]
path = "/var/lib/hoard/volumes"

[s3]
endpoint   = "http://minio:9000"
bucket     = "backups"
region     = "us-east-1"
access_key = "${S3_ACCESS_KEY}"
secret_key = "${S3_SECRET_KEY}"
prefix     = "hoard"

# Fallback for files not matching any volume
[defaults]
ttl = "7d"
retries = 3
extensions = ["*"]

[[storage_classes]]
name = "long-term"
ttl = "90d"
retries = 5
compression = "zstd"
encryption = false

[[storage_classes]]
name = "short-term"
ttl = "3d"
retries = 2

[[volumes]]
name = "postgres"
match = "postgres/**"
storage_class = "long-term"
s3_prefix = "databases/postgres"
extensions = ["db", "wal"]
on_stop = "drain"
on_delete = "keep"

[[volumes]]
name = "app-logs"
match = "app-logs/**"
storage_class = "short-term"
s3_prefix = "logs/app"
extensions = ["log", "json"]
on_stop = "drain"

[[volumes]]
name = "catch-all"
match = "**"
storage_class = "short-term"
s3_prefix = "misc"
```

### Volume resolution

When a file is written at `/var/lib/hoard/volumes/postgres/schema/v2.sql`:

1. Match volumes in declaration order
2. `postgres/**` matches → `storage_class = "long-term"`, `s3_prefix = "databases/postgres"`
3. S3 key: `databases/postgres/schema/v2.sql`

More specific globs win when both match. Declaration order breaks ties.

---

## Field reference

### `[daemon]`

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `mode` | `"standalone"` or `"nomad"` | `"standalone"` | no | Runtime mode |
| `service` | string | `"default"` | no | Logical service name. Used for control socket path `/run/hoard/<service>.sock` |
| `control_socket` | path | `/run/hoard/<service>.sock` | no | Unix domain socket for `hoard ctl` |
| `metrics_addr` | `host:port` | `0.0.0.0:9150` | no | Prometheus metrics + health endpoint |

### `[watch]`

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `path` | path | — | **yes** | Root directory to watch recursively |

### `[s3]`

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `endpoint` | URL | — | **yes** | S3-compatible endpoint (e.g. `http://minio:9000`) |
| `bucket` | string | — | **yes** | S3 bucket name |
| `region` | string | `"us-east-1"` | no | AWS region (required for SigV4) |
| `access_key` | string | — | no | S3 access key |
| `secret_key` | string | — | no | S3 secret key |
| `prefix` | string | `""` | no | S3 key prefix (e.g. `"hoard"` → `hoard/path/to/file`) |
| `no_sign` | bool | `false` | no | Skip SigV4 signing (for MinIO without auth) |

### `[gc]`

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `interval_secs` | u64 | `21600` (6h) | no | How often to run garbage collection |
| `ttl_days` | u64 | `30` | no | Delete S3 objects older than this |

### `[filter]`

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `extensions` | `[string]` | `["*"]` | no | File extensions to replicate (without dot: `"db"`, `"log"`) |
| `exclude` | `[string]` | `[]` | no | Glob patterns to exclude (e.g. `"*.tmp"`, `"*.journal"`) |

### `[resilience]`

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `pending_db` | path | `/var/lib/hoard/pending.db` | no | SQLite database for pending upload persistence |
| `max_upload_retries` | u32 | `5` | no | Retries per file before dead-letter |
| `dead_letter_dir` | path | `/var/lib/hoard/dead-letter` | no | Failed uploads moved here after max retries |

### `[nomad]` (v1 only; v2 uses env vars)

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `addr` | URL | `http://127.0.0.1:4646` | no | Nomad API address |
| `token` | string | — | no | Nomad ACL token |

### StorageClass fields (v2)

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `name` | string | — | **yes** | Unique class identifier |
| `ttl` | duration | `"30d"` | no | Object TTL (`"7d"`, `"90d"`, `"365d"`) |
| `retries` | u32 | `5` | no | Upload retry count |
| `compression` | `"zstd"` or none | none | no | Compress before upload |
| `encryption` | bool | `false` | no | (reserved) |
| `extensions` | `[string]` | `["*"]` | no | File extensions for this class |

### Volume fields (v2)

| Field | Type | Default | Required | Description |
|-------|------|---------|----------|-------------|
| `name` | string | — | **yes** | Volume identifier |
| `match` | glob | — | **yes** | Path glob relative to watch root |
| `storage_class` | string | — | no | Reference to a `[[storage_classes]]` name |
| `s3_prefix` | string | `""` | no | Override S3 prefix for this volume |
| `extensions` | `[string]` | (from class) | no | Override extensions |
| `exclude` | `[string]` | `[]` | no | Glob patterns to skip |
| `ttl` | duration | (from class) | no | Override TTL |
| `retries` | u32 | (from class) | no | Override retry count |
| `on_stop` | `"drain"` or `"keep"` or `"purge"` | `"drain"` | no | Behavior when volume is removed from config |
| `on_delete` | `"keep"` or `"purge"` | `"keep"` | no | Behavior when S3 objects are GC'd |

### Environment variable mapping (v1)

Every TOML key can be set via env var. The convention is:

| TOML path | Env var |
|-----------|---------|
| `daemon.mode` | `HOARD_MODE` |
| `watch.path` | `HOARD_WATCH_ROOT` |
| `s3.endpoint` | `HOARD_S3_ENDPOINT` |
| `s3.bucket` | `HOARD_S3_BUCKET` |
| `s3.access_key` | `HOARD_S3_ACCESS_KEY` |
| `s3.secret_key` | `HOARD_S3_SECRET_KEY` |
| `s3.prefix` | `HOARD_S3_PREFIX` |
| `s3.no_sign` | `HOARD_S3_NO_SIGN` |
| `s3.region` | `HOARD_S3_REGION` |
| `filter.extensions` | `HOARD_FILTER_EXTENSIONS` (comma-separated) |
| `gc.interval_secs` | `HOARD_GC_INTERVAL_SECS` |
| `gc.ttl_days` | `HOARD_GC_TTL_DAYS` |
| `nomad.addr` | `HOARD_NOMAD_ADDR` |
| `nomad.token` | `HOARD_NOMAD_TOKEN` |

{: .tip }
You can mix env vars and TOML. Env vars override TOML values at startup.

### config file discovery

Hoard searches for config in this order:

1. `--config <path>` flag (explicit)
2. `HOARD_CONFIG` env var
3. `/etc/hoard/hoard.toml`
4. `./hoard.toml` (working directory)

v2 additionally watches `/etc/hoard/conf.d/*.toml` for hot-reload.
