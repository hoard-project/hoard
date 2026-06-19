job "stress-large" {
  type = "batch"
  datacenters = ["dc1"]
  group "writers" {
    count = 2
    task "large-writer" {
      driver = "raw_exec"
      config {
        command = "/bin/bash"
        args = [
          "-c",
          "set -eu\nD=/var/lib/hoard/volumes/stress-large\nmkdir -p $D\nt1=$(($(date +%s)+120))\nwhile [ $(date +%s) -lt $t1 ]; do\n  dd if=/dev/urandom of=$D/big-${NOMAD_ALLOC_INDEX}-$(date +%s).bin bs=1M count=$((RANDOM%3+2)) 2>/dev/null\n  sleep 10\ndone\necho alloc-${NOMAD_ALLOC_INDEX}: done"
        ]
      }
      resources {
        cpu    = 100
        memory = 64
      }
    }
  }
}
