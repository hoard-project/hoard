# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

**DO NOT open a public issue.**  
Email: `hoard-project.team@zvip.eu.org`

We aim to acknowledge within 48 hours and provide a fix within 7 days.

## Security Model

Hoard runs as a **privileged daemon** (BPF + raw socket access). It:

- Loads eBPF programs into the kernel (requires `CAP_BPF` / `CAP_SYS_ADMIN`)
- Opens raw TCP sockets for kTLS uploads
- Optionally reads Nomad's Unix socket for lifecycle events

### What We Protect Against

- Credential leaks: all secrets via environment variables, never in config files
- Supply chain: Dependabot + CodeQL + `cargo audit` on every PR
- Memory safety: `#![deny(unsafe_code)]` in all modules except `src/ffi.rs`

### What We Do NOT Protect Against

- A compromised kernel — if the attacker has `CAP_SYS_ADMIN`, you have bigger problems
- Physical access to the Nomad host
