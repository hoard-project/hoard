# Hoard 集群储存底座压测 Job 集
#
# 使用方式:
#   nomad job run stress-sqlite.nomad
#   nomad job run -var='count=10' stress-logs.nomad
#   nomad job run stress-large.nomad
#   nomad job run stress-json.nomad
#
# 清理:
#   nomad job stop -purge stress-sqlite
#   nomad job stop -purge stress-logs
#   nomad job stop -purge stress-large
#   nomad job stop -purge stress-json
