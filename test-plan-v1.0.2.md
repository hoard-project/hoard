# Hoard v1.0.2 — 全量功能测试计划

## 测试架构

```
┌──────────────────────────────────────────────────────────────────┐
│                      gentle-sample (212.60.153.53)               │
│                                                                  │
│  ┌───────────────────┐  ┌───────────────────┐  ┌──────────────┐ │
│  │ Hoard daemon      │  │ MinIO :9000       │  │ Garage :3900  │ │
│  │ (raw_exec on      │  │ (Docker)          │  │ (bare-metal)  │ │
│  │  Nomad)           │  │                   │  │               │ │
│  │                   │  │ bucket:           │  │ bucket:       │ │
│  │ S3 backend:       │  │ guardian-backups  │  │ hoard-stress  │ │
│  │  MinIO + Garage   │  └───────────────────┘  └──────────────┘ │
│  └────────┬──────────┘                                          │
│           │ eBPF hook (VFS layer)                                │
│           ▼                                                     │
│  ┌────────────────────────────────────────────────────────────┐ │
│  │ watch_paths: /tmp/hoard-test/                               │ │
│  │   ├── mixed/       (SQLite + txt + binary)                  │ │
│  │   ├── churn/       (high-frequency writes, stress)          │ │
│  │   ├── classes/     (StorageClass routing test)              │ │
│  │   └── large/       (64k–1M files)                           │ │
│  └────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

## M1–M7 测试阶段

| 阶段 | 功能域 | 测试项 | 后端 |
|------|--------|--------|------|
| M1 | 基础健康 | 1–7 | MinIO |
| M2 | 文件检测/上传 | 8–15 | MinIO |
| M3 | 压缩+签名 | 16–21 | MinIO → Garage |
| M4 | GC/死信/重试 | 22–28 | MinIO |
| M5 | 配置/路由 | 29–36 | MinIO |
| M6 | Metrics/控制 | 37–43 | MinIO |
| M7 | 压测 | S1–S6 | MinIO + Garage |

---

## M1: 基础健康检查

| # | 测试 | 预期 |
|---|------|------|
| 1 | `--version` 返回 v1.0.2 | "hoard 1.0.2" |
| 2 | `--check-bpf` BPF 加载成功 | "BPF program loaded successfully" |
| 3 | 守护进程启动 (v2 TOML) | 无 panic，PID 存活 |
| 4 | S3 HeadBucket 连通 | bucket exists / warning 403 |
| 5 | control socket 绑定 | /var/run/hoard.sock 存在，0600 |
| 6 | metrics 端点响应 | `curl :9150/metrics` → 200 |
| 7 | health 端点 | `GET :9150/health` → `{"status":"ok"}` |

## M2: 文件检测与上传

| # | 测试 | 预期 |
|---|------|------|
| 8 | SQLite WAL 写入检测 | 1 个 .db/.db-wal 写入 → S3 可见 |
| 9 | 纯文本文件写入 | 5 个 .txt 写入 → 全部上传 |
| 10 | 二进制文件写入 | 5 个 .bin 写入 → 全部上传 |
| 11 | 多文件并发写入 | 20 文件同时 touch → 全部检测 |
| 12 | 扩展名过滤 | `extensions = ["db","txt"]` → 仅匹配上传 |
| 13 | 排除模式 | `exclude = ["*.tmp"]` → .tmp 不上传 |
| 14 | sendfile 零拷贝路径 | 无压缩上传 → ETag 校验通过 |
| 15 | eBPF debounce 100ms | 快速连续写 → 只产生 1 次上传 |

## M3: 压缩、签名、多后端

| # | 测试 | 预期 |
|---|------|------|
| 16 | zstd 压缩上传 (MinIO) | .zst 后缀，下载解压原文一致 |
| 17 | zstd PUT presigned URL | SigV4 presigned URL → 200 |
| 18 | Garage SigV4 连通 | presigned URL PUT → Garage 200 |
| 19 | MinIO → Garage 对比 | 同文件两后端 ETag 一致 |
| 20 | 压缩比验证 | zstd 输出 < 原始大小 |
| 21 | 混合压缩/不压缩 | 部分 volume zstd，部分 none → 全部成功 |

## M4: GC、死信、重试

| # | 测试 | 预期 |
|---|------|------|
| 22 | 手动 GC flush | `curl -X POST :9150/flush` → 200 |
| 23 | TTL 过期删除 | `ttl="1s"` → GC 周期后对象删除 |
| 24 | GC dry-run 安全 | 无 `on_delete="delete"` → 不删文件 |
| 25 | 死信队列生成 | S3 不可达 → 文件入 /var/lib/hoard/dead-letter |
| 26 | 死信队列指标 | `hoard_dead_letter_files` > 0 |
| 27 | 指数退避重试 | delay 1s→2s→4s→8s→16s |
| 28 | 重试耗尽入死信 | max_retries=3 → 3 次后入 dead-letter |

## M5: 配置/卷路由

| # | 测试 | 预期 |
|---|------|------|
| 29 | defaults 继承 | 不匹配 class → 用 defaults |
| 30 | StorageClass 路由 | `name="long-term"` → ttl=90d |
| 31 | Volume 精确匹配 | `match="postgres/**"` → 匹配路由 |
| 32 | Volume 通配 catch-all | `match="**"` → 全部匹配 |
| 33 | Volume 优先级 | 长匹配优先于短匹配 |
| 34 | s3_prefix 隔离 | 不同 volume → 不同 S3 prefix |
| 35 | 环境变量展开 | `${S3_BUCKET}` → 展开为实际值 |
| 36 | CLI 覆盖 TOML | `--watch-path` flag → 覆盖配置 |

## M6: Metrics 和 Control API

| # | 测试 | 预期 |
|---|------|------|
| 37 | `hoard_upload_total` | Counter 累加正确 |
| 38 | `hoard_upload_bytes_total` | 字节计数正确 |
| 39 | `hoard_pending_files` | flush 后归零 |
| 40 | `hoard_etag_mismatch_total` | 正常情况 = 0 |
| 41 | `hoard_ringbuf_events_total` | 写入后递增 |
| 42 | `hoard_health_status` | = 1 (healthy) |
| 43 | `/health` JSON 端点 | `{"status":"ok"}` |

## M7: 压测架构

```
┌─────────────────────────────────────────────────────────┐
│                  压测矩阵 (per backend)                   │
│                                                         │
│  S1: 高并发小文件 (4K × 200 files, 10 writers)          │
│  S2: 大文件 (1M × 20 files)                             │
│  S3: 混合大小 (4K/64K/256K/1M × 各25 = 100 files)      │
│  S4: WAL 高频 (10 writes/s × 60s, SQLite)               │
│  S5: 长尾持续 (1000 files, 5分钟间隔, 持续30分钟)       │
│  S6: 恢复/容错 (kill -9 + restart, 验证无数据丢失)     │
│                                                         │
│  判定标准: upload_total ≥ found, etag_mismatch = 0,     │
│           dead_letter = 0, health = 1                   │
└─────────────────────────────────────────────────────────┘
```
