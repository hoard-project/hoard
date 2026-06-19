job "stress-json" {
  type = "batch"
  datacenters = ["dc1"]
  group "writer" {
    count = 1
    task "json-writer" {
      driver = "raw_exec"
      config {
        command = "/bin/bash"
        args = [
          "-c",
          "FILE=/var/lib/hoard/volumes/stress-json/data.json\nCNT=0\nEND=$((SECONDS+120))\nwhile [ $SECONDS -lt $END ]; do\n  printf '{\"alloc\": \"%s\", \"ts\": \"%s\", \"cnt\": %d}\n' \"${NOMAD_ALLOC_INDEX}\" \"$(date -Is)\" $CNT | hoard-atomic $FILE\n  CNT=$((CNT+1))\n  sleep 2\ndone\necho alloc-${NOMAD_ALLOC_INDEX}: $CNT overwrites"
        ]
      }
      resources {
        cpu    = 50
        memory = 32
      }
    }
  }
}
