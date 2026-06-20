# Control socket API

!!! note
    Only available in `standalone` mode. Nomad mode uses HTTP endpoints instead.

| Property | Value |
|----------|-------|
| Path | `/run/hoard/{service}.sock` (default: `/run/hoard/default.sock`) |
| Protocol | Raw text, newline-delimited |
| Encoding | UTF-8 |

## Protocol

**Request**: single line of text followed by `\n`.

**Response**: single line of text.

```
flush\n       → "ok" | "error: ..."
status\n      → JSON status object
```

## Commands

### `flush`

Trigger an immediate upload drain (same as `POST /flush`).

```
// Send
flush

// Response
ok
```

### `status`

Query daemon metrics.

```
// Send
status

// Response
{
  "pending": 3,
  "dead_letter": 0,
  "uploads_total": 15420,
  "errors": 0,
  "bytes_uploaded": 4294967296
}
```

## Usage examples

```bash
# Flush
echo flush | nc -U /run/hoard/default.sock

# Status
echo status | nc -U /run/hoard/default.sock

# Or use hoardctl
hoardctl flush default
hoardctl status default
```

## HTTP endpoints (Nomad mode / metrics server)

| Method | Path | Response |
|--------|------|----------|
| `GET` | `/metrics` | Prometheus OpenMetrics text format |
| `GET` | `/health` | `{"status":"ok"\|"degraded","pending":N,"dead_letter":N}` |
| `POST` / `GET` | `/flush` | `{"status":"ok","message":"flush triggered"}` |
| `POST` | `/nomad-drain?timeout=N` | `{"status":"ok","pending":0,"wait_ms":N}` |
