# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 1.x     | :white_check_mark: |
| 0.6.x   | :white_check_mark: |
| 0.5.x   | :warning: (EOL)    |
| < 0.5   | :x:                |

## Reporting a Vulnerability

**DO NOT open a public issue.**  
Email: `hoard-project.team@zvip.eu.org`

We aim to acknowledge within 48 hours and provide a fix within 7 days.

## Security Model

Hoard runs as a **privileged daemon** (BPF + raw socket access). It:

- Loads eBPF programs into the kernel (requires `CAP_BPF` / `CAP_SYS_ADMIN`)
- Opens TCP sockets for S3 uploads
- Optionally exposes a Unix domain socket for control commands

### What We Protect Against

- Credential leaks: all secrets via environment variables or Vault, never hardcoded
- Supply chain: Dependabot + CodeQL on every PR
- Memory safety: `#![deny(unsafe_code)]` in all modules except `src/ffi.rs` and `src/bin/hoard_atomic.rs`
- File integrity: `pread(2)` TOCTOU-safe MD5 verification before `sendfile(2)` upload

### What We Do NOT Protect Against

- A compromised kernel — if the attacker has `CAP_SYS_ADMIN`, you have bigger problems
- Physical access to the Nomad host
