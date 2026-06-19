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
          "DIR=/var/lib/hoard/volumes/stress-large\nmkdir -p $DIR\nEND=$((SECONDS+120))\nwhile [ $SECONDS -lt $END ]; do\n  SZ=$((RANDOM%3+2))\n  dd if=/dev/urandom of=$DIR/big-${NOMAD_ALLOC_INDEX}-$(date +%s).bin bs=1M count=$SZ 2>/dev/null\n  echo wrote ${SZ}MB\n  sleep 10\ndone\necho alloc-${NOMAD_ALLOC_INDEX}: done"
        ]
      }
      resources {
        cpu    = 100
        memory = 64
      }
    }
  }
}
