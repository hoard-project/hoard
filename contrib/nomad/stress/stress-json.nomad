job "stress-json" {
  type = "batch"
  datacenters = ["dc1"]
  parameterized { payload = "forbidden" }
  group "writer" {
    count = 1
    task "json-writer" {
      driver = "raw_exec"
      config {
        command = "/bin/bash"
        args = ["-c", <<EOF
FILE="/var/lib/hoard/volumes/stress-json/state.json"
mkdir -p "$(dirname "$FILE")"
END=$((SECONDS + 120))
while [ $SECONDS -lt $END ]; do
  printf '{"ts":"%s","random":%d,"uptime":%d}\n' "$(date -Iseconds)" $RANDOM $SECONDS > "$FILE"
  sleep 2
done
echo "json-writer: done"
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
