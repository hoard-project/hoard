// Hoard — Self-bootstrapping deployment via GitHub Release artifacts
//
// Downloads binary + BPF object from GitHub Releases at alloc time.
// No pre-installed binaries needed on the host.
//
// Prerequisites:
//   - Linux kernel ≥ 5.5
//   - Nomad cluster with internet egress (github.com)
//   - raw_exec driver enabled in client config
//
// Deploy:
//   nomad run hoard-artifact.nomad
//
// Override S3 credentials:
//   nomad run -var 's3_access_key=xxx' -var 's3_secret_key=yyy' hoard-artifact.nomad
//
// Upgrade version:
//   nomad run -var 'hoard_version=0.6.5' hoard-artifact.nomad

variable "hoard_version" {
  type        = string
  default     = "0.6.5"
  description = "Hoard version (must match a GitHub Release tag, e.g. 0.6.5)"
}

variable "s3_access_key" {
  type        = string
  default     = ""
  description = "S3 access key"
}

variable "s3_secret_key" {
  type        = string
  default     = ""
  description = "S3 secret key"
}

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

    # ── Pre-start: download & install binary + BPF object ──
    task "install" {
      driver = "raw_exec"
      lifecycle {
        hook    = "prestart"
        sidecar = false
      }

      config {
        command = "/bin/sh"
        args = ["-c", <<-SCRIPT
          set -e
          BIN=/usr/local/bin/hoard
          BPF=/usr/lib/hoard/hoard.bpf.o
          mkdir -p /usr/lib/hoard /var/lib/hoard/volumes

          # Binary
          cp local/hoard-bin/hoard-x86_64 "$BIN"
          chmod +x "$BIN"

          # BPF object
          cp local/hoard-bin/hoard-x86_64.bpf.o "$BPF"

          echo "hoard installed: $($BIN --version)"
        SCRIPT
        ]
      }

      artifact {
        source      = "https://github.com/hoard-project/hoard/releases/download/v${var.hoard_version}/hoard-x86_64"
        destination = "local/hoard-bin/"
      }

      artifact {
        source      = "https://github.com/hoard-project/hoard/releases/download/v${var.hoard_version}/hoard-x86_64.bpf.o"
        destination = "local/hoard-bin/"
      }

      resources {
        cpu    = 50
        memory = 32
      }
    }

    # ── Main daemon ──
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
service     = "{{ env "NOMAD_ALLOC_NAME" }}"

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
        S3_ACCESS_KEY = var.s3_access_key
        S3_SECRET_KEY = var.s3_secret_key
        S3_PREFIX     = "hoard"
      }

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