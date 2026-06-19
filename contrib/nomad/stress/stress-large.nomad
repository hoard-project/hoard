job "stress-large" {
  type = "batch"
  datacenters = ["dc1"]
  parameterized { payload = "forbidden" }
  group "writers" {
    count = 2
    task "large-writer" {
      driver = "raw_exec"
      config {
        command = "/bin/bash"
        args = ["-c", <<EOF
DIR="/var/lib/hoard/volumes/stress-large"
mkdir -p "$DIR"
END=$((SECONDS + 120))
while [ $SECONDS -lt $END ]; do
  SIZE=$(((RANDOM % 3) + 2))
  dd if=/dev/urandom of="$DIR/big-${NOMAD_ALLOC_INDEX}-$(date +%s%N).bin" bs=1M count=$SIZE 2>/dev/null
  echo "wrote ${SIZE}MB"
  sleep 10
done
EOF
        ]
      }
      resources {
        cpu    = 100
        memory = 64
      }
    }
  }
}
