---
title: Quickstart
nav_order: 2
---

# Quickstart

{: .note }
This guide assumes a Linux host with kernel ≥ 5.5. Check: `uname -r`

## 1. Install

Download the latest binary and BPF object from GitHub Releases:

```bash
curl -sL https://github.com/hoard-project/hoard/releases/latest/download/hoard-x86_64 \
  -o /usr/local/bin/hoard
curl -sL https://github.com/hoard-project/hoard/releases/latest/download/hoard-x86_64.bpf.o \
  -o /usr/lib/hoard/hoard.bpf.o
chmod +x /usr/local/bin/hoard
mkdir -p /usr/lib/hoard
```

Verify:

```bash
hoard --version
# hoard 0.6.5
```

## 2. Start a local S3 (optional)

If you don't have an S3 backend, start MinIO:

```bash
docker run -d --name minio \
  -p 9000:9000 -p 9001:9001 \
  -e MINIO_ROOT_USER=minioadmin \
  -e MINIO_ROOT_PASSWORD=minioadmin123 \
  minio/minio:latest server /data --console-address ":9001"

# Create a bucket
mc alias set local http://127.0.0.1:9000 minioadmin minioadmin123
mc mb local/hoard-backups
```

## 3. Create a watch directory

```bash
mkdir -p /var/lib/hoard/volumes
```

Any file written under this tree will be detected by the BPF hooks and
uploaded to S3.

## 4. Run Hoard

### Option A: env vars (simplest)

```bash
HOARD_MODE=standalone \
HOARD_WATCH_ROOT=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=http://127.0.0.1:9000 \
HOARD_S3_BUCKET=hoard-backups \
HOARD_S3_ACCESS_KEY=minioadmin \
HOARD_S3_SECRET_KEY=minioadmin123 \
HOARD_S3_PREFIX=hoard \
HOARD_S3_NO_SIGN=true \
  hoard
```

### Option B: TOML config

```bash
cat > /etc/hoard/hoard.toml << 'EOF'
[daemon]
mode = "standalone"

[watch]
path = "/var/lib/hoard/volumes"

[s3]
endpoint   = "http://127.0.0.1:9000"
bucket     = "hoard-backups"
access_key = "minioadmin"
secret_key = "minioadmin123"
prefix     = "hoard"
no_sign    = true
EOF

hoard --config /etc/hoard/hoard.toml
```

## 5. Verify it works

Write a test file:

```bash
echo "hello hoard" > /var/lib/hoard/volumes/test.txt
```

After ~30 seconds (the periodic drain interval), check S3:

```bash
mc ls local/hoard-backups/hoard/
# [2026-06-20 ...] 11B test.txt
```

Check health:

```bash
curl http://127.0.0.1:9150/health
# {"status":"ok"}
```

Force an immediate flush (standalone mode only):

```bash
hoard ctl flush default
```

## 6. Run as a systemd service

```bash
cp contrib/hoard.service /etc/systemd/system/
cp contrib/hoard.toml.example /etc/hoard/hoard.toml
# Edit /etc/hoard/hoard.toml with your S3 credentials
systemctl enable --now hoard
```

## Next

- [Configuration](configuration) — full config reference (v1 + v2)
- [Operations](operations) — restore, metrics, health, troubleshooting
- [Nomad Deployment](nomad) — run as a Nomad system job
