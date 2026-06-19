job "stress-sqlite" {
  type = "batch"
  datacenters = ["dc1"]
  group "writers" {
    count = 50
    task "sqlite-writer" {
      driver = "raw_exec"
      config {
        command = "/bin/bash"
        args = [
          "-c",
          "set -e\nDB=/var/lib/hoard/volumes/stress-sqlite/alloc-${NOMAD_ALLOC_INDEX}.db\nmkdir -p $(dirname $DB)\nsqlite3 $DB \"CREATE TABLE IF NOT EXISTS t(id INTEGER PRIMARY KEY, ts TEXT, val REAL);\"\nCOUNT=0\nEND=$((SECONDS+120))\nwhile [ $SECONDS -lt $END ]; do\n  sqlite3 $DB \"INSERT INTO t(ts,val) VALUES(datetime(),random()/1000000.0);\"\n  COUNT=$((COUNT+1))\n  sleep 0.5\ndone\necho alloc-${NOMAD_ALLOC_INDEX}: $COUNT inserts"
        ]
      }
      resources {
        cpu    = 20
        memory = 32
      }
    }
  }
}
