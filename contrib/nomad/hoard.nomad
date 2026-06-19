// Hoard — eBPF file-change replication daemon (Nomad system job)
//
// Deploys Hoard on every client node. Watches /var/lib/hoard/volumes
// and replicates file changes to S3-compatible storage.
//
// Prerequisites:
//   - Linux kernel ≥ 5.5 (BPF trampoline support)
//   - BPF capabilities: CAP_BPF + CAP_SYS_ADMIN (or run as root)
//   - S3-compatible endpoint (MinIO, Garage, AWS, …)
//   - hoard binary at /usr/local/bin/hoard + BPF object at /usr/lib/hoard/hoard.bpf.o
//   - raw_exec driver enabled in client config
//
// If raw_exec is not available, switch driver to "exec" and add a
// host volume mount (see hoard-artifact.nomad for volume example).
//
// Usage:
//   nomad run hoard.nomad
//
// S3 credentials should be provided via Vault or Nomad variables,
// NOT hardcoded in this file.

job "hoard" {
  datacenters = ["dc1"]
  type        = "system"
  priority    = 90

  constraint {
    attribute = "${attr.kernel.name}"
    value     = "linux"
  }

  constraint {
    attribute = "${attr.kernel.version}"
    operator  = "semver"
    value     = ">= 5.5.0"
  }

  group "hoard" {
    stop_after_client_disconnect = "30s"

    task "hoard" {
      driver = "raw_exec"

      config {
        command = "/usr/local/bin/hoard"
        args    = ["--config", "local/hoard.toml"]
      }

      template {
        data = <<EOF
[daemon]
mode        = "standalone"
service     = "hoard"

[watch]
path = "/var/lib/hoard/volumes"

[s3]
endpoint   = "{{ env "S3_ENDPOINT" }}"
region     = "{{ env "S3_REGION" }}"
bucket     = "{{ env "S3_BUCKET" }}"
access_key = "{{ env "S3_ACCESS_KEY" }}"
secret_key = "{{ env "S3_SECRET_KEY" }}"
prefix     = "{{ env "S3_PREFIX" }}"

[gc]
interval_secs = 21600
ttl_days      = 30

[resilience]
pending_db        = "/var/lib/hoard/pending.db"
max_upload_retries = 5
dead_letter_dir   = "/var/lib/hoard/dead-letter"

[filter]
extensions = ["db", "sqlite", "sqlite3", "wal", "log", "json", "csv"]
exclude    = ["*.tmp", "*.journal"]
EOF
        destination = "local/hoard.toml"
        change_mode = "signal"
        change_signal = "SIGHUP"
      }

      env {
        S3_ENDPOINT   = "http://127.0.0.1:9000"
        S3_REGION     = "us-east-1"
        S3_BUCKET     = "guardian-backups"
        S3_ACCESS_KEY = ""   # REQUIRED — set via Vault or nomad variable
        S3_SECRET_KEY = ""   # REQUIRED — set via Vault or nomad variable
        S3_PREFIX     = "hoard"
      }

      # ── Vault integration (optional — uncomment and comment out env{} above) ──
      # This replaces the static env{} block with Vault-backed secrets.
      #
      # Prerequisites:
      #   1. Vault cluster accessible from Nomad clients
      #   2. Enable kv-v2:  vault secrets enable -path=hoard kv-v2
      #   3. Store creds:   vault kv put hoard/s3 access_key=xxx secret_key=yyy
      #   4. Create policy: vault policy write hoard-s3-read - <<<'path "hoard/data/s3" { capabilities = ["read"] }'
      #
      # vault {
      #   policy = "hoard-s3-read"
      # }
      # template {
      #   data        = <<EOF
      # S3_ACCESS_KEY={{ with secret "hoard/data/s3" }}{{ .Data.data.access_key }}{{ end }}
      # S3_SECRET_KEY={{ with secret "hoard/data/s3" }}{{ .Data.data.secret_key }}{{ end }}
      # EOF
      #   destination = "secrets/s3.env"
      #   env         = true
      # }

      resources {
        cpu    = 100
        memory = 64
      }

      kill_timeout = "30s"

      restart {
        interval = "5m"
        attempts = 3
        delay    = "15s"
        mode     = "delay"
      }
    }
  }
}