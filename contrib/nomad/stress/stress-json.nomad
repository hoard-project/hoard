// stress-json.nomad — JSON 覆盖写入压测（ETag 竞争窗口验证）
//
// 每 2s 覆盖写入同一个 JSON 文件。此场景触发 Hoard 的
// ETag 校验竞争窗口：文件在 sendfile 和 md5 计算间可能被改写。
// 用于验证 #17 修复后 health 不因 TOCTOU ETag 而降级（bonus）。
//
// dispatch: nomad job run stress-json.nomad

job "stress-json" {
  type = "batch"
  datacenters = ["dc1"]

  parameterized {
    payload = "forbidden"
    meta_required = []
  }

  group "writer" {
    count = 1

    task "json-writer" {
      driver = "raw_exec"

      config {
        command = "/bin/sh"
        args = [
          "-c",
          <<-EOF
          set -e
          FILE="/var/lib/hoard/volumes/stress-json/state.json"
          mkdir -p "$(dirname "$FILE")"
          END=$((SECONDS + 120))
          while [ $SECONDS -lt $END ]; do
            cat > "$FILE" <<JSON
{"ts":"$$(date -Iseconds)","random":$$RANDOM,"uptime":$$SECONDS,"wr":"stress"}
JSON
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
