// Hoard — Self-bootstrapping deployment via GitHub Release artifacts
//
// Downloads binary + BPF object from GitHub Releases at alloc time.
// No pre-installed binaries needed on the host.
//
// Prerequisites:
//   - Linux kernel ≥ 5.5
//   - Nomad cluster with internet egress (github.com)
//   - xz utility on host (for decompression)
//   - raw_exec driver enabled in client config
//
// Deploy:
//   nomad run hoard-artifact.nomad
//
// Override S3 credentials:
//   nomad run -var 's3_access_key=xxx' -var 's3_secret_key=yyy' hoard-artifact.nomad
//
// Upgrade version:
//   Edit HOARD_VERSION below and re-run:
//   nomad run hoard-artifact.nomad

variable "hoard_version" {
  type        = string
  default     = "0.3.1"
  description = "Hoard version to deploy (must match a GitHub Release tag without 'v' prefix)"
}

variable "s3_access_key" {
  type        = string
  default     = ""
  description = "S3 access key — override with: nomad run -var 's3_access_key=xxx'"
}

variable "s3_secret_key" {
  type        = string
  default     = ""
  description = "S3 secret key — override with: nomad run -var 's3_secret_key=yyy'"
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
          mkdir -p /usr/lib/hoard

          xz -d -f local/hoard-bin/hoard-linux-amd64.xz -c > "$BIN.tmp"
          mv "$BIN.tmp" "$BIN"
          chmod +x "$BIN"

          xz -d -f local/hoard-bin/hoard-bpf-x86.o.xz -c > "$BPF.tmp"
          mv "$BPF.tmp" "$BPF"

          echo "hoard installed: $($BIN --version 2>/dev/null || echo ok)"
        SCRIPT
        ]
      }

      artifact {
        source      = "https://github.com/hoard-project/hoard/releases/download/v${var.hoard_version}/hoard-linux-amd64.xz"
        destination = "local/hoard-bin/"
      }

      artifact {
        source      = "https://github.com/hoard-project/hoard/releases/download/v${var.hoard_version}/hoard-bpf-x86.o.xz"
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
        S3_PREFIX      = "hoard"
      }

      resources {
        cpu    = 100
        memory = 64
      }

      kill_timeout = "30s"

      restart_policy {
        interval = "5m"
        attempts = 3
        delay    = "15s"
        mode     = "delay"
      }
    }
  }
}