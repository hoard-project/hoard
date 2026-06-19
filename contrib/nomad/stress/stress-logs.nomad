// stress-logs.nomad — 并发日志写入压测
//
// 每个 alloc 每秒 1 行日志，持续 120s。模拟 Nginx/应用日志场景。
// 默认 10 实例，可参数化。
//
// dispatch: nomad job run stress-logs.nomad
// dispatch: nomad job run -var='count=5' stress-logs.nomad

variable "count" {
  type    = number
  default = 10
}

job "stress-logs" {
  type = "batch"
  datacenters = ["dc1"]

  parameterized {
    payload = "forbidden"
    meta_required = []
  }

  group "writers" {
    count = var.count

    task "log-writer" {
      driver = "raw_exec"

      config {
        command = "/bin/sh"
        args = [
          "-c",
          <<-EOF
          set -e
          LOG="/var/lib/hoard/volumes/stress-logs/app-${NOMAD_ALLOC_INDEX}.log"
          mkdir -p "$(dirname "$LOG")"
          END=$((SECONDS + 120))
          while [ $SECONDS -lt $END ]; do
            echo "[$$(date -Iseconds)] alloc=$${NOMAD_ALLOC_INDEX} status=200 latency=$$((RANDOM%50))ms path=/api/v$$((RANDOM%10))" >> "$LOG"
            sleep 1
          done
          echo "log-writer-$${NOMAD_ALLOC_INDEX}: done, $$(wc -l < "$LOG") lines"
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
