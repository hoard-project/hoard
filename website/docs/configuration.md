# Configuration

!!! tip "Priority"
    CLI flag > env var > TOML value > default

## v1 Quick config

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

## v2: StorageClass + Volume model

!!! note
    v2 config uses `conf.d/` directory hot-reload. Place `.toml` files in
    `/etc/hoard/conf.d/` — Hoard watches this directory and reloads on change.

### Concepts

| Concept | Analogous to | Purpose |
|---------|-------------|---------|
| **StorageClass** | K8s StorageClass | Reusable policy (TTL, retries, compression) |
| **Volume** | K8s PVC | Path glob → StorageClass + S3 prefix |
| **defaults** | StorageClass fallback | Applied when no volume matches |

### Example

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
access_key = "${S3_ACCESS_KEY}"
secret_key = "${S3_SECRET_KEY}"
prefix     = "hoard"

[defaults]
ttl = "7d"
retries = 3
extensions = ["*"]

[[storage_classes]]
name = "long-term"
ttl = "90d"
retries = 5
compression = "zstd"

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

[[volumes]]
name = "app-logs"
match = "app-logs/**"
storage_class = "short-term"
s3_prefix = "logs/app"

[[volumes]]
name = "catch-all"
match = "**"
storage_class = "short-term"
s3_prefix = "misc"
```

### Resolution

When a file is written at `.../volumes/postgres/schema/v2.sql`:

1. Match volumes in declaration order
2. `postgres/**` matches → class `long-term`, prefix `databases/postgres`
3. S3 key: `databases/postgres/schema/v2.sql`

More specific globs win. Declaration order breaks ties.

## Field reference

### `[daemon]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `"standalone" \| "nomad"` | `"standalone"` | Runtime mode |
| `service` | string | `"default"` | Service name (socket path) |
| `metrics_addr` | `host:port` | `0.0.0.0:9150` | Prometheus endpoint |

### `[s3]`

| Field | Type | Default | Required |
|-------|------|---------|----------|
| `endpoint` | URL | — | **yes** |
| `bucket` | string | — | **yes** |
| `region` | string | `us-east-1` | no |
| `access_key` | string | — | no |
| `secret_key` | string | — | no |
| `prefix` | string | `""` | no |
| `no_sign` | bool | `false` | no |

### `[gc]`

| Field | Type | Default |
|-------|------|---------|
| `interval_secs` | u64 | `21600` (6h) |
| `ttl_days` | u64 | `30` |

### `[filter]`

| Field | Type | Default |
|-------|------|---------|
| `extensions` | `[string]` | `["*"]` |
| `exclude` | `[string]` | `[]` |

### `[resilience]`

| Field | Type | Default |
|-------|------|---------|
| `pending_db` | path | `/var/lib/hoard/pending.db` |
| `max_upload_retries` | u32 | `5` |
| `dead_letter_dir` | path | `/var/lib/hoard/dead-letter` |

### StorageClass (v2)

| Field | Type | Default | Required |
|-------|------|---------|----------|
| `name` | string | — | **yes** |
| `ttl` | duration | `"30d"` | no |
| `retries` | u32 | `5` | no |
| `compression` | `"zstd"` \| none | none | no |
| `encryption` | bool | `false` | no |

### Volume (v2)

| Field | Type | Required |
|-------|------|----------|
| `name` | string | **yes** |
| `match` | glob | **yes** |
| `storage_class` | string | no |
| `s3_prefix` | string | no |
| `extensions` | `[string]` | no |
| `exclude` | `[string]` | no |
| `ttl` | duration | no |
| `retries` | u32 | no |
| `on_stop` | `"drain" \| "keep" \| "purge"` | no |
| `on_delete` | `"keep" \| "purge"` | no |

## Config file discovery

1. `--config <path>` flag (explicit)
2. `HOARD_CONFIG` env var
3. `/etc/hoard/hoard.toml`
4. `./hoard.toml` (working directory)

v2 additionally watches `/etc/hoard/conf.d/*.toml`.
