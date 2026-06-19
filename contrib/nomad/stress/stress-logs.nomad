job "stress-logs" {
  type = "batch"
  datacenters = ["dc1"]
  parameterized { payload = "forbidden" }
  group "writers" {
    count = 10
    task "log-writer" {
      driver = "raw_exec"
      config {
        command = "/bin/bash"
        args = ["-c", <<EOF
LOG="/var/lib/hoard/volumes/stress-logs/app-${NOMAD_ALLOC_INDEX}.log"
mkdir -p "$(dirname "$LOG")"
END=$((SECONDS + 120))
while [ $SECONDS -lt $END ]; do
  echo "[$(date -Iseconds)] alloc=${NOMAD_ALLOC_INDEX} status=200 latency=$((RANDOM%50))ms path=/api/v$((RANDOM%10))" >> "$LOG"
  sleep 1
done
echo "log-${NOMAD_ALLOC_INDEX}: done, $(wc -l < "$LOG") lines"
EOF
        ]
      }
      resources {
        cpu    = 20
        memory = 16
      }
    }
  }
}
