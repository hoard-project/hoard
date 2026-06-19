// stress-large.nomad — 大文件 sendfile 通路压测
//
// 2 个 alloc 各自每 10s 生成一个 2~5 MB 随机二进制文件。
// 此文件通过 Hoard 的 sendfile 零拷贝通路上传到 S3。
//
// dispatch: nomad job run stress-large.nomad

job "stress-large" {
  type = "batch"
  datacenters = ["dc1"]

  parameterized {
    payload = "forbidden"
    meta_required = []
  }

  group "writers" {
    count = 2

    task "large-writer" {
      driver = "raw_exec"

      config {
        command = "/bin/sh"
        args = [
          "-c",
          <<-EOF
          set -e
          DIR="/var/lib/hoard/volumes/stress-large"
          mkdir -p "$DIR"
          END=$((SECONDS + 120))
          while [ $SECONDS -lt $END ]; do
            SIZE=$(( (RANDOM % 3) + 2 ))
            dd if=/dev/urandom of="$DIR/big-$${NOMAD_ALLOC_INDEX}-$$$$(date +%s%N).bin" bs=1M count=$SIZE 2>/dev/null
            echo "alloc-$${NOMAD_ALLOC_INDEX}: wrote $${SIZE}MB"
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
