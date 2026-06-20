---
title: Nomad Deployment
nav_order: 6
---

# Nomad Deployment

Run Hoard as a Nomad system job — one instance per cluster node.

---

## Prerequisites

| Requirement | Minimum |
|-------------|---------|
| Nomad | ≥ 1.8 (SSE events) |
| Driver | `exec` (recommended) or `raw_exec` |
| Kernel | ≥ 5.5 with BTF |

{: .important }
**exec vs raw_exec**: `exec` creates a network namespace, enabling CNI.
`raw_exec` runs directly on the host. For Hoard, either works.

---

## Job spec

```hcl
job "hoard" {
  type = "system"

  group "hoard" {
    task "hoard" {
      driver = "exec"

      config {
        command = "/usr/local/bin/hoard"
        args    = ["--config", "${NOMAD_TASK_DIR}/hoard.toml"]
      }

      artifact {
        source = "https://github.com/hoard-project/hoard/releases/download/v0.6.5/hoard-x86_64"
        destination = "local/hoard"
        mode = "file"
      }

      artifact {
        source = "https://github.com/hoard-project/hoard/releases/download/v0.6.5/hoard-x86_64.bpf.o"
        destination = "local/hoard.bpf.o"
        mode = "file"
      }

      template {
        data = <<EOF
[daemon]
mode = "standalone"

[watch]
path = "/var/lib/hoard/volumes"

[s3]
endpoint   = "{{ with secret "secret/hoard/s3" }}{{ .Data.data.endpoint }}{{ end }}"
bucket     = "{{ with secret "secret/hoard/s3" }}{{ .Data.data.bucket }}{{ end }}"
access_key = "{{ with secret "secret/hoard/s3" }}{{ .Data.data.access_key }}{{ end }}"
secret_key = "{{ with secret "secret/hoard/s3" }}{{ .Data.data.secret_key }}{{ end }}"
prefix     = "hoard"
EOF
        destination = "${NOMAD_TASK_DIR}/hoard.toml"
      }

      # Copy BPF object into expected location
      template {
        data = <<EOF
#!/bin/sh
mkdir -p /usr/lib/hoard
cp {{ env "NOMAD_TASK_DIR" }}/hoard.bpf.o /usr/lib/hoard/hoard.bpf.o
cp {{ env "NOMAD_TASK_DIR" }}/hoard /usr/local/bin/hoard
chmod +x /usr/local/bin/hoard
mkdir -p /var/lib/hoard/volumes
EOF
        destination = "local/setup.sh"
        perms = "755"
      }

      lifecycle {
        hook    = "prestart"
        sidecar = false
      }

      resources {
        cpu    = 100
        memory = 64
      }

      kill_timeout = "30s"
    }
  }
}
```

{: .note }
The `kill_timeout = "30s"` is critical. It gives Hoard time to drain
pending uploads before Nomad force-kills the allocation.

---

## Production considerations

### Secrets management

Do NOT hardcode S3 credentials in job specs. Use one of:

1. **Nomad Variables** (v1.9+):
   ```hcl
   template {
     data = <<EOF
   access_key = "{{ with nomadVar "hoard/s3" }}{{ .access_key }}{{ end }}"
   EOF
   }
   ```

2. **Vault** (shown in the example above)

### Volume mount

If your applications write to specific directories, mount them into
the Hoard task:

```hcl
volume "app-data" {
  type   = "host"
  source = "app_data"
  read_only = true
}

task "hoard" {
  volume_mount {
    volume      = "app-data"
    destination = "/var/lib/hoard/volumes/app"
    read_only   = true
  }
}
```

### Resource tuning

| Workload | CPU | Memory | Notes |
|----------|-----|--------|-------|
| Light (logs only) | 50 | 32 | BPF polling is lightweight |
| Medium (databases) | 100 | 64 | WAL checkpoint adds CPU |
| Heavy (large files + many volumes) | 200 | 128 | sendfile is I/O bound, not CPU bound |

### Monitoring

Add a Prometheus scrape target for each Hoard instance. Use Nomad
service discovery:

```hcl
service {
  name = "hoard-metrics"
  port = "metrics"
  tags = ["prometheus"]
}
```

---

## Contrib Nomad files

See [`contrib/nomad/`]({{ site.baseurl }}/../contrib/nomad/) for:

| File | Purpose |
|------|---------|
| `hoard.nomad` | Basic system job (exec driver) |
| `hoard-artifact.nomad` | Job with GitHub Release artifact download |
| `hoard-vars.nomad` | Job using Nomad Variables for secrets |
| `sqlite-service.nomad` | Example: Hoard + SQLite app co-location |

### Stress test jobs

[`contrib/nomad/stress/`]({{ site.baseurl }}/../contrib/nomad/stress/) contains
job specs for load testing:

| File | What it tests |
|------|---------------|
| `stress-sqlite.nomad` | Concurrent SQLite writers |
| `stress-logs.nomad` | Log file rotation |
| `stress-large.nomad` | Large file sendfile |
| `stress-json.nomad` | JSON file throughput |

---

## Multi-node verification

After deploying, verify all nodes:

```bash
# Check allocations
nomad job status hoard

# Check health on each node
for node in node1 node2 node3; do
  echo "=== $node ==="
  curl -s http://$node:9150/health
done
```
