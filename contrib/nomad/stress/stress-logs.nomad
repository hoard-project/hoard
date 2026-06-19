job "stress-logs" {
  type = "batch"
  datacenters = ["dc1"]
  group "writers" {
    count = 10
    task "log-writer" {
      driver = "raw_exec"
      config {
        command = "/bin/bash"
        args = [
          "-c",
          "DIR=/var/lib/hoard/volumes/stress-logs\nmkdir -p $DIR\nCNT=0\nEND=$((SECONDS+120))\nwhile [ $SECONDS -lt $END ]; do\n  echo [$(date -Is)] alloc=${NOMAD_ALLOC_INDEX} cnt=$CNT status=200 >> $DIR/access.log\n  CNT=$((CNT+1))\n  sleep $((RANDOM%3+1))\ndone\necho alloc-${NOMAD_ALLOC_INDEX}: $CNT lines"
        ]
      }
      resources {
        cpu    = 20
        memory = 32
      }
    }
  }
}
