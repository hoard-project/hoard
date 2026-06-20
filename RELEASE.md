# Release Process

1. **Bump version** in `Cargo.toml` (e.g. `1.0.0`)
2. **Update CHANGELOG.md**: move `Unreleased` to the new version header
3. **Open PR** with version bump + changelog; get CI green
4. **Merge to main**
5. **Create signed tag**:
   ```bash
   git tag -s v1.0.0 -m "v1.0.0"
   git push origin v1.0.0
   ```
6. **GitHub Actions** triggers [`release.yml`](.github/workflows/release.yml):
   - Builds `x86_64` and `aarch64` binaries
   - Strips + sha256 hashes
   - Creates GitHub Release with 8 assets:
     ```
     hoard-x86_64
     hoard-x86_64.sha256
     hoard-x86_64.bpf.o
     hoard-x86_64.bpf.o.sha256
     hoard-aarch64
     hoard-aarch64.sha256
     hoard-aarch64.bpf.o
     hoard-aarch64.bpf.o.sha256
     ```
   - Auto-generates release notes from merged PRs
7. **Announce** in GitHub Discussions

## Versioning

Follow [Semantic Versioning](https://semver.org/).

- **MAJOR**: breaking API/config changes
- **MINOR**: new features, backward-compatible
- **PATCH**: bug fixes only

## Security

If a release contains a security fix, follow the process in
[SECURITY.md](SECURITY.md) — embargo until the fix is publicly available.
