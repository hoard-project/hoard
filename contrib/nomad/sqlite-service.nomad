// Example application job — SQLite database with Hoard backup
//
// Demonstrates:
//   1. Pre-start restore from S3 (db-restore sidecar)
//   2. Application with shared volume watched by Hoard
//   3. Clean integration with the system-level hoard job
//
// NOTE: Hoard runs as a *system* job (see hoard.nomad), NOT as a
// sidecar. It watches the host directory /var/lib/hoard/volumes
// independently. This job just mounts a host volume into that path.
//
// Usage:
//   nomad run sqlite-service.nomad

job "sqlite-service" {
  datacenters = ["dc1"]
  type        = "service"

  group "db-group" {
    count = 1

    // Host volume shared with the system-level hoard job
    volume "shared_disk" {
      type      = "host"
      source    = "hoard_storage"
      read_only = false
    }

    // Pre-start: restore latest database backup from S3
    task "db-restore" {
      lifecycle {
        hook    = "prestart"
        sidecar = false
      }

      driver = "exec"

      config {
        command = "/usr/local/bin/hoard"
        args    = [
          "restore",
          "--s3-endpoint",  "${S3_ENDPOINT}",
          "--s3-bucket",    "${S3_BUCKET}",
          "--s3-prefix",    "hoard",
          "--s3-access-key", "${S3_ACCESS_KEY}",
          "--s3-secret-key", "${S3_SECRET_KEY}",
          "--dest",         "/var/lib/hoard/volumes/${NOMAD_JOB_NAME}",
        ]
      }

      volume_mount {
        volume      = "shared_disk"
        destination = "/var/lib/hoard/volumes"
      }

      env {
        S3_ENDPOINT   = "http://127.0.0.1:9000"
        S3_BUCKET     = "guardian-backups"
        S3_ACCESS_KEY = ""    # REQUIRED — set via Vault or nomad variable
        S3_SECRET_KEY = ""    # REQUIRED — set via Vault or nomad variable
      }

      resources {
        cpu    = 50
        memory = 32
      }
    }

    // Main application task
    task "backend" {
      driver = "exec"

      config {
        command = "/usr/local/bin/my-sqlite-app"
      }

      volume_mount {
        volume      = "shared_disk"
        destination = "/app/database"
        sub_dir     = "${NOMAD_JOB_NAME}/${NOMAD_ALLOC_ID}"
      }

      kill_timeout = "30s"

      resources {
        cpu    = 200
        memory = 128
      }
    }
  }
}