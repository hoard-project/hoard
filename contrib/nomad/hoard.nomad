# Nomad system job — deploys Hoard on every node
# Usage: nomad run hoard.nomad

job "hoard" {
  datacenters = ["dc1"]
  type        = "system"
  priority    = 90

  group "hoard" {
    task "hoard" {
      driver = "raw_exec"

      config {
        command = "/usr/local/bin/hoard"
        args = [
          "--mode",           "nomad",
          "--nomad-addr",     "unix:///opt/nomad/data/client/agent.sock",
          "--watch-root",     "/var/lib/hoard/volumes",
          "--tls-mode",       "ktls",
          "--s3-endpoint",    "${S3_ENDPOINT}",
          "--s3-region",      "${S3_REGION}",
          "--s3-bucket",      "${S3_BUCKET}",
          "--s3-access-key",  "${S3_ACCESS_KEY}",
          "--s3-secret-key",  "${S3_SECRET_KEY}",
          "--gc-interval",    "21600",
          "--gc-ttl-days",    "7",
        ]
      }

      env {
        S3_ENDPOINT   = "https://s3.amazonaws.com"
        S3_REGION     = "us-east-1"
        S3_BUCKET     = "hoard-backup"
        S3_ACCESS_KEY = ""  # set via Nomad variable or Vault
        S3_SECRET_KEY = ""  # set via Nomad variable or Vault
      }

      resources {
        cpu    = 50
        memory = 10
      }

      kill_timeout = "30s"
    }
  }
}
