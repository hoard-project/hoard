# Nomad service job — example SQLite application with Hoard integration
# Usage: nomad run sqlite-service.nomad

job "sqlite-service" {
  datacenters = ["dc1"]
  type        = "service"

  group "db-group" {
    count = 1

    volume "shared_disk" {
      type      = "host"
      source    = "hoard_storage"
      read_only = false
    }

    # Prestart: restore database from S3 before the app starts
    task "db-restore" {
      lifecycle {
        hook    = "prestart"
        sidecar = false
      }

      driver = "exec"

      config {
        command = "/usr/local/bin/hoardctl"
        args    = [
          "restore",
          "--job-name",  "${NOMAD_JOB_NAME}",
          "--alloc-id",  "${NOMAD_ALLOC_ID}",
        ]
      }

      volume_mount {
        volume      = "shared_disk"
        destination = "/var/lib/hoard/volumes"
      }

      env {
        HOARD_S3_ENDPOINT   = "https://s3.amazonaws.com"
        HOARD_S3_REGION     = "us-east-1"
        HOARD_S3_BUCKET     = "hoard-backup"
        HOARD_S3_ACCESS_KEY = ""  # set via Nomad variable or Vault
        HOARD_S3_SECRET_KEY = ""  # set via Nomad variable or Vault
      }

      resources {
        cpu    = 50
        memory = 32
      }
    }

    # Main application task
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

      meta {
        hoard_watch    = "true"
        hoard_patterns = "*.db,*.db-wal,*.db-shm"
      }

      kill_timeout = "30s"

      resources {
        cpu    = 200
        memory = 128
      }
    }
  }
}
