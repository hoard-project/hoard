# Quickstart

!!! note "Kernel requirement"
    Linux kernel ≥ 5.5 with BTF. Check: `uname -r` and `ls /sys/kernel/btf/vmlinux`

## 1. Install

=== "GitHub Release"

    ```bash
    curl -sL https://github.com/hoard-project/hoard/releases/latest/download/hoard-x86_64       -o /usr/local/bin/hoard
    curl -sL https://github.com/hoard-project/hoard/releases/latest/download/hoard-x86_64.bpf.o       -o /usr/lib/hoard/hoard.bpf.o
    chmod +x /usr/local/bin/hoard
    mkdir -p /usr/lib/hoard
    ```

=== "Build from source"

    ```bash
    git clone https://github.com/hoard-project/hoard
    cd hoard
    cargo build --release
    sudo cp target/release/hoard /usr/local/bin/
    BPF=$(find target/release/build -name hoard.bpf.o | head -1)
    sudo mkdir -p /usr/lib/hoard
    sudo cp "$BPF" /usr/lib/hoard/hoard.bpf.o
    ```

Verify:

```bash
hoard --version
# hoard 1.0.2
```

## 2. Start local S3 (optional)

```bash
docker run -d --name s3   -p 9000:9000 -p 9001:9001   -e MINIO_ROOT_USER=s3admin   -e MINIO_ROOT_PASSWORD=s3admin123   minio/minio:latest server /data --console-address ":9001"

mc alias set local http://127.0.0.1:9000 s3admin s3admin123
mc mb local/hoard-backups
```

## 3. Create watch directory

```bash
mkdir -p /var/lib/hoard/volumes
```

## 4. Run

```bash
HOARD_MODE=standalone \
HOARD_WATCH_PATH=/var/lib/hoard/volumes \
HOARD_S3_ENDPOINT=http://127.0.0.1:9000 \
HOARD_S3_BUCKET=hoard-backups \
HOARD_S3_ACCESS_KEY=s3admin \
HOARD_S3_SECRET_KEY=s3admin123 \
HOARD_S3_NO_SIGN=true \
  hoard
```

## 5. Verify

```bash
echo "hello hoard" > /var/lib/hoard/volumes/test.txt
# Wait 30s for periodic drain...

mc ls local/hoard-backups/hoard/
# [2026-06-20 ...] 11B test.txt

curl http://127.0.0.1:9150/health
# {"status":"ok"}
```

## 6. systemd (production)

```bash
cp contrib/hoard.service /etc/systemd/system/
cp contrib/hoard.toml.example /etc/hoard/hoard.toml
# Edit /etc/hoard/hoard.toml with your S3 credentials
systemctl enable --now hoard
```
