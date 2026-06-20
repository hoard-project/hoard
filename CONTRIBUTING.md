# Contributing to Hoard

Thanks for your interest in contributing.

## Before you start

- Read the [architecture docs](https://hoard-project.github.io/hoard/architecture/).
- Check [open issues](https://github.com/hoard-project/hoard/issues) — look for `good first issue`.
- For large changes, open a **design discussion** issue first.

## Development environment

```bash
git clone https://github.com/hoard-project/hoard
cd hoard

# Install Rust toolchain (MSRV 1.82)
rustup default stable

# Build
cargo build --release

# Run tests
cargo test

# Run clippy
cargo clippy -- -D warnings

# Check formatting
cargo fmt --check
```

**Kernel requirements**: Linux ≥ 5.5 with BTF. The BPF object is CO-RE — one
binary works across kernel versions. BPF tests require `CAP_BPF` (run as root
or with `sudo`).

## Project structure

```
src/
├── main.rs          # Entry point
├── hoard.rs         # Lifecycle state machine
├── bpf/             # eBPF C source (aya-ebpf)
├── ebpf/            # BPF loader, debounce, filter, inode resolution
├── upload/          # sendfile pipeline, retry, outcome
├── s3/              # SigV4 signing, PutObject, GC
├── config/          # v1/v2 config, env parsing, clap CLI
├── metrics.rs       # Observability metrics (prometheus crate)
├── pending.rs       # SQLite pending-set persistence
├── ffi.rs           # Unsafe FFI (kernel/libc calls only)
├── fd.rs            # SocketFd wrapper
└── verify.rs        # ETag verification
```

## Code conventions

- `#![deny(unsafe_code)]` on every module except `ffi.rs` and `src/bin/hoard_atomic.rs`
- SAFETY comments required on all unsafe blocks
- Clippy is mandatory (`cargo clippy -- -D warnings`)
- `cargo fmt` before committing
- Tests live alongside source in `#[cfg(test)] mod tests`
- Doc comments (`///`) on all public APIs

## Commit style

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat: add recursive file discovery
fix: handle zero-length uploads
docs: update quickstart guide
chore: update dependencies
```

## Pull request checklist

- [ ] Tests pass (`cargo test`)
- [ ] Clippy clean (`cargo clippy -- -D warnings`)
- [ ] Formatted (`cargo fmt --check`)
- [ ] BPF object builds (`cargo build --release`)
- [ ] CHANGELOG.md updated (if user-facing)
- [ ] Documentation updated (if API changes)

## License

All contributions are licensed under GPL-3.0 (see [LICENSE](LICENSE)).
