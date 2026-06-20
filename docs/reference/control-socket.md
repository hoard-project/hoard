---
title: Control Socket API
nav_order: 8
parent: Reference
---

# Control Socket API

The control socket is a Unix domain socket at `/run/hoard/<service>.sock`
(standalone mode).

{: .note }
Nomad mode does not expose a control socket. Use Nomad's alloc API instead.

---

## Wire protocol

Newline-delimited JSON over Unix stream socket. One request per connection.

**Request format**:

```json
{"command": "<COMMAND>", "args": {}}
```

**Response format**:

```json
{"status": "ok", "data": {}}
```

Error:

```json
{"status": "error", "error": "message"}
```

---

## Commands

### `status`

Query daemon health and counters.

**Request**:

```json
{"command": "status"}
```

**Response**:

```json
{
  "status": "ok",
  "data": {
    "service": "default",
    "mode": "standalone",
    "health": "ok",
    "pending_files": 0,
    "total_uploads": 1523,
    "dead_letter_files": 0,
    "uptime_secs": 86400
  }
}
```

### `flush`

Trigger immediate drain of pending uploads.

**Request**:

```json
{"command": "flush"}
```

**Response**:

```json
{
  "status": "ok",
  "data": {
    "uploaded": 3,
    "failed": 0
  }
}
```

### `restore`

Restore files from S3.

**Request**:

```json
{
  "command": "restore",
  "args": {
    "target": "/tmp/restored",
    "prefix": "databases/postgres",
    "dry_run": false
  }
}
```

**Response**:

```json
{
  "status": "ok",
  "data": {
    "restored": 1523,
    "bytes": 1288490188,
    "dry_run": false
  }
}
```

### `reload`

Hot-reload configuration (v2 only).

**Request**:

```json
{"command": "reload"}
```

**Response**:

```json
{
  "status": "ok",
  "data": {
    "volumes_reloaded": 3,
    "classes_reloaded": 2
  }
}
```

---

## Example: health check in shell

```bash
echo '{"command":"status"}' | nc -U /run/hoard/default.sock | python3 -m json.tool
```

```bash
#!/bin/sh
# nagios/icinga health check
RESP=$(echo '{"command":"status"}' | nc -U /run/hoard/default.sock)
HEALTH=$(echo "$RESP" | python3 -c "import sys,json; print(json.load(sys.stdin)['data']['health'])")
if [ "$HEALTH" = "ok" ]; then
    echo "OK: hoard health=$HEALTH"
    exit 0
else
    echo "CRITICAL: hoard health=$HEALTH"
    exit 2
fi
```
