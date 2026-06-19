job "hoard" {
  datacenters = ["dc1"]
  type        = "system"
  priority    = 90

  group "hoard" {
    stop_after_client_disconnect = "30s"

    task "hoard" {
      driver = "raw_exec"

      config {
        command = "/usr/local/bin/hoard"
        args    = ["--config", "/etc/hoard/hoard.toml"]
      }

      resources {
        cpu    = 2000
        memory = 256
      }
    }
  }
}
