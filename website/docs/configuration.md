# Configuration

!!! tip "Priority"
    CLI flag > env var > TOML value > default

## v2 TOML schema

```toml
[hoard]
version = 2

[daemon]
mode = "standalone"
service = "production"
metrics_addr = "0.0.0.0:9150"

[watch]
paths = ["/var/lib/hoard/volumes"]

[s3]
endpoint   = "http://s3:9000"
region     = "us-east-1"
bucket     = "backups"
access_key = "${S3_ACCESS_KEY}"
secret_key = "${S3_SECRET_KEY}"
no_sign    = false

[defaults]
prefix      = "hoard"
ttl         = "7d"
retries     = 3
extensions  = ["*"]
exclude     = ["*.tmp", "*.journal"]
compression = "zstd"   # zstd level 3; S3 key gets .zst suffix
encryption  = false    # (planned)
on_stop     = "drain"
on_delete   = "keep"

[resilience]
pending_db         = "/var/lib/hoard/pending.db"
max_upload_retries = 5
dead_letter_dir    = "/var/lib/hoard/dead-letter"

[nomad]
addr           = ""
token          = ""
meta_enabled   = false
meta_poll_secs = 300

[[storage_classes]]
name = "long-term"
ttl = "90d"
retries = 5
compression = "zstd"
# encryption = true  # (planned)

[[storage_classes]]
name = "short-term"
ttl = "3d"
retries = 2

[[volumes]]
name = "postgres"
match = "postgres/**"
class = "long-term"
s3_prefix = "databases/postgres"
extensions = ["db", "wal"]
on_stop = "drain"
enabled = true

[[volumes]]
name = "app-logs"
match = "app-logs/**"
class = "short-term"
s3_prefix = "logs/app"

[[volumes]]
name = "catch-all"
match = "**"
s3_prefix = "misc"
```

!!! warning "No `[gc]` section"
    GC interval and TTL are set via environment variables
    `HOARD_GC_INTERVAL` / `HOARD_GC_TTL_DAYS` or CLI flags
    `--gc-interval` / `--gc-ttl-days`. There is no `[gc]` section
    in the v2 TOML schema.

### Concepts

| Concept | Analogous to | Purpose |
|---------|-------------|---------|
| **StorageClass** | K8s StorageClass | Reusable policy (TTL, retries, compression) |
| **Volume** | K8s PVC | Path glob → StorageClass + S3 prefix |
| **defaults** | StorageClass fallback | Applied when no volume matches |

### Resolution

When a file is written at `volumes/postgres/schema/v2.sql`:

1. Match volumes in declaration order
2. `postgres/**` matches → class `long-term`, prefix `databases/postgres`
3. S3 key: `databases/postgres/schema/v2.sql`

More specific globs win. Declaration order breaks ties.

## Field reference

### `[hoard]`

| Field | Type | Required |
|-------|------|----------|
| `version` | u32 | **yes** — must be `2` |
| `conf_dirs` | `[string]` | no — extra config directories |

### `[daemon]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `"standalone"` \| `"nomad"` | `"standalone"` | Runtime mode |
| `service` | string | `"default"` | Service name (socket path) |
| `metrics_addr` | `host:port` | `0.0.0.0:9150` | Prometheus endpoint |
| `control_socket` | path | `/var/run/hoard.sock` | Unix socket for hoardctl |

### `[watch]`

| Field | Type | Required |
|-------|------|----------|
| `paths` | `[string]` | **yes** — at least one watch directory |

### `[s3]`

| Field | Type | Default | Required |
|-------|------|---------|----------|
| `endpoint` | URL | — | **yes** |
| `bucket` | string | — | **yes** |
| `region` | string | `""` | no |
| `access_key` | string | `""` | no |
| `secret_key` | string | `""` | no |
| `no_sign` | bool | `false` | no |

### `[defaults]`

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `prefix` | string | `"default"` | S3 key prefix for unmatched volumes |
| `ttl` | duration | `"30d"` | Retention before GC |
| `retries` | u32 | `5` | Upload retry count |
| `extensions` | `[string]` | `["*"]` | Watch file extensions (e.g. `["db","wal"]`) |
| `exclude` | `[string]` | `["*.tmp", "*.journal"]` | Patterns to exclude |
| `compression` | `"zstd"` \| none | none | zstd level 3; S3 key gets `.zst` |
| `encryption` | bool | `false` | *(planned)* at-rest encryption |
| `on_stop` | `"drain"` \| `"keep"` \| `"purge"` | `"drain"` | Action when volume is removed |
| `on_delete` | `"keep"` \| `"purge"` | `"keep"` | Action when watched file is deleted |

### GC settings

!!! note "Environment variables / CLI only"
    GC is configured outside the TOML schema.

| Env var | CLI flag | Default |
|---------|----------|---------|
| `HOARD_GC_INTERVAL` | `--gc-interval <SECS>` | `3600` (1h) |
| `HOARD_GC_TTL_DAYS` | `--gc-ttl-days <DAYS>` | `30` |

### StorageClass

| Field | Type | Default | Required |
|-------|------|---------|----------|
| `name` | string | — | **yes** |
| `ttl` | duration | `"30d"` | no |
| `retries` | u32 | `5` | no |
| `compression` | `"zstd"` \| none | none | zstd level 3 |
| `encryption` | bool | `false` | *(planned)* |
| `on_stop` | `"drain"` \| `"keep"` \| `"purge"` | inherit | no |
| `on_delete` | `"keep"` \| `"purge"` | inherit | no |

### Volume

| Field | Type | Required |
|-------|------|----------|
| `name` | string | **yes** |
| `match` | glob | **yes** |
| `class` | string | no |
| `s3_prefix` | string | no |
| `extensions` | `[string]` | no |
| `exclude` | `[string]` | no |
| `ttl` | duration | no |
| `retries` | u32 | no |
| `compression` | `"zstd"` \| none | no |
| `encryption` | bool | no — *(planned)* |
| `on_stop` | `"drain"` \| `"keep"` \| `"purge"` | no |
| `on_delete` | `"keep"` \| `"purge"` | no |
| `enabled` | bool | `true` | Set `false` to disable |

### `[resilience]`

| Field | Type | Default |
|-------|------|---------|
| `pending_db` | path | `/var/lib/hoard/pending.db` |
| `max_upload_retries` | u32 | `5` |
| `dead_letter_dir` | path | `/var/lib/hoard/dead-letter` |

### `[nomad]`

| Field | Type | Default |
|-------|------|---------|
| `addr` | URL | `""` |
| `token` | string | `""` |
| `meta_enabled` | bool | `false` |
| `meta_poll_secs` | u64 | `300` |

## Config file discovery

1. `--config <path>` flag (explicit)
2. `HOARD_CONFIG` env var
3. `/etc/hoard/hoard.toml`
4. `./hoard.toml` (working directory)

v2 additionally loads `/etc/hoard/conf.d/*.toml` at startup and on SIGHUP
(config reload).  These files support `[[storage_classes]]` and `[[volumes]]`
sections only.
