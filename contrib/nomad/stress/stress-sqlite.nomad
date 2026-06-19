// stress-sqlite.nomad — 50× 并发 SQLite 写入压测
//
// 每个 alloc 持续 120s，每秒 ~2 次 INSERT。全部写入完成后
// Hoard 的 BPF fentry/vfs_write 捕获 → debounce → sendfile 上传 S3。
//
// dispatch: nomad job run stress-sqlite.nomad
// 修改实例数: nomad job run -var='count=20' stress-sqlite.nomad

job "stress-sqlite" {
  type = "batch"
  datacenters = ["dc1"]

  parameterized {
    payload = "forbidden"
    meta_required = []
  }

  group "writers" {
    count = 50

    task "sqlite-writer" {
      driver = "raw_exec"

      config {
        command = "/bin/sh"
        args = [
          "-c",
          <<-EOF
          set -e
          DB="/var/lib/hoard/volumes/stress-sqlite/alloc-${NOMAD_ALLOC_INDEX}.db"
          mkdir -p "$(dirname "$DB")"
          sqlite3 "$DB" "CREATE TABLE IF NOT EXISTS t(id INTEGER PRIMARY KEY, ts TEXT, val REAL);"
          COUNT=0
          END=$((SECONDS + 120))
          while [ $SECONDS -lt $END ]; do
            sqlite3 "$DB" "INSERT INTO t(ts,val) VALUES(datetime('now'), random()/1000000.0);"
            COUNT=$((COUNT + 1))
            sleep 0.5
          done
          echo "alloc-${NOMAD_ALLOC_INDEX}: $COUNT inserts"
          EOF
        ]
      }

      resources {
        cpu    = 20
        memory = 32
      }
    }
  }
}
