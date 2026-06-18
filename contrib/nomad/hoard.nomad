// Hoard — eBPF file-change replication daemon (Nomad system job)
//
// Deploys Hoard on every client node. Watches /var/lib/hoard/volumes
// and replicates file changes to S3-compatible storage.
//
// Prerequisites:
//   - Linux kernel ≥ 5.5 (BPF trampoline support)
//   - BPF capabilities: CAP_BPF + CAP_SYS_ADMIN (or run as root)
//   - S3-compatible endpoint (MinIO, Garage, AWS, …)
//   - hoard binary + hoard.bpf.o on the host or via artifact
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

  // Spread allocations across all eligible nodes
  constraint {
    attribute = "${os.kernel.name}"
    value     = "linux"
  }

  // BPF requires kernel ≥ 5.5 — reject ancient hosts
  constraint {
    attribute = "${kernel.version}"
    operator  = "semver"
    value     = ">= 5.5.0"
  }

  group "hoard" {
    // Graceful shutdown: allow pending uploads to drain
    stop_after_client_disconnect = "30s"

    network {
      // Prometheus metrics
      port "metrics" {
        static = 9150
      }
    }

    task "hoard" {
      driver = "exec"

      // Binary must be pre-installed at /usr/local/bin/hoard on the host
      // with the BPF object at /usr/lib/hoard/hoard.bpf.o
      // For artifact-based deployment, see hoard-artifact.nomad
      config {
        command = "/usr/local/bin/hoard"
        args = [
          "--config", "local/hoard.toml",
        ]
      }

      // Config template — all env vars are interpolated by Nomad
      template {
        data        = <<EOF
[daemon]
mode        = "standalone"
service     = "{{ env "NOMAD_ALLOC_NAME" }}"
metrics_addr = "0.0.0.0:9150"

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
        S3_ACCESS_KEY = ""    # REQUIRED — set via Vault or nomad variable
        S3_SECRET_KEY = ""    # REQUIRED — set via Vault or nomad variable
        S3_PREFIX      = "hoard"
      }

      resources {
        cpu    = 100   # BPF + ringbuf polling is lightweight; 100 MHz headroom
        memory = 64    # ~30 MB RSS typical, 64 MB headroom for spikes
      }

      // Allow time for graceful drain on SIGTERM
      kill_timeout = "30s"

      // Restart policy: BPF failures or S3 blips should self-heal
      restart_policy {
        interval     = "5m"
        attempts     = 3
        delay        = "15s"
        mode         = "delay"
      }
    }
  }
}