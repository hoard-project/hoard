---
sidebar_position: 9
---

# Control socket API

:::note
Only available in `standalone` mode. Nomad mode has no control socket.
:::

| Property | Value |
|----------|-------|
| Path | `/var/run/hoard.sock` |
| Protocol | JSON-RPC over Unix domain datagram |
| Encoding | UTF-8, one JSON object per datagram |

## Protocol

**Request**: single-line JSON object with `action` field.

```json
{"action": "<COMMAND>", ...}
```

**Response**: single-line JSON object. Always includes `status`.

```json
{"status": "ok", ...}
```

```json
{"status": "error", "message": "..."}
```

## Commands

### `ping`

```json
// Request
{"action": "ping"}

// Response
{"status": "ok", "uptime_secs": 86400, "version": "1.0.0-beta.1"}
```

### `status`

```json
// Request
{"action": "status"}

// Response
{
  "status": "ok",
  "pending_files": 12,
  "uploads_total": 15420,
  "upload_errors": 3,
  "bytes_uploaded": 4294967296,
  "dead_letter_count": 1,
  "bpf_events_total": 98432,
  "last_drain_at": "2026-06-20T06:55:00Z",
  "last_gc_at": "2026-06-20T00:00:00Z"
}
```

### `list`

```json
// Request — list all files for a path
{"action": "list", "path": "/var/lib/hoard/volumes/postgres"}

// Response
{
  "status": "ok",
  "path": "/var/lib/hoard/volumes/postgres",
  "files": [
    {"key": "postgres/schema/v2.sql", "size": 2048, "last_modified": "2026-06-20T06:50:00Z"},
    {"key": "postgres/data/main.db", "size": 1048576, "last_modified": "2026-06-20T06:49:00Z"}
  ]
}
```

### `restore`

```json
// Request — restore single file from S3 to local path
{"action": "restore", "path": "/var/lib/hoard/volumes/postgres/schema/v2.sql"}

// Response
{"status": "ok", "path": "/tmp/hoard-restore/schema/v2.sql", "size": 2048}
```

### `gc`

```json
// Request — trigger garbage collection
{"action": "gc"}

// Response
{"status": "ok", "deleted": 15, "errors": 0, "freed_bytes": 47395676}
```

### `drain`

```json
// Request — force immediate pending drain
{"action": "drain"}

// Response
{"status": "ok", "uploaded": 12, "errors": 0}
```

### `reprocess`

```json
// Request — reprocess a dead-letter file
{"action": "reprocess", "path": "dead-letter.txt"}

// Response
{"status": "ok", "uploaded": true}
```

### `reprocess_all`

```json
// Request — reprocess all dead-letter files
{"action": "reprocess_all"}

// Response
{"status": "ok", "reprocessed": 3, "errors": 1}
```

## Usage examples

```bash
# Ping
echo '{"action":"ping"}' | nc -U /var/run/hoard.sock | jq .

# Status
echo '{"action":"status"}' | nc -U /var/run/hoard.sock | jq .

# Trigger drain
echo '{"action":"drain"}' | nc -U /var/run/hoard.sock | jq .

# List all S3 objects under postgres/
echo '{"action":"list","path":"/var/lib/hoard/volumes/postgres"}' \
  | nc -U /var/run/hoard.sock | jq .

# Restore specific file
echo '{"action":"restore","path":"/var/lib/hoard/volumes/postgres/schema/v2.sql"}' \
  | nc -U /var/run/hoard.sock | jq .
```
