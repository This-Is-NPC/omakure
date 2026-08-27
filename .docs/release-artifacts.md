# Release artifact format

The release workflow builds one headless `omakure` executable per supported
platform. It does not package workspace scripts, themes, TUI assets, or widget
files.

## Archives

- `omakure-vX.Y.Z-linux-x86_64.tar.gz` (dynamically linked against glibc)
- `omakure-vX.Y.Z-linux-musl-x86_64.tar.gz` (statically linked)
- `omakure-vX.Y.Z-darwin-x86_64.tar.gz`
- `omakure-vX.Y.Z-windows-x86_64.zip`

`install.sh` prefers the `linux-musl` archive and falls back to the glibc one
when a release predates it. Omakure is installed on machines the operator does
not control, and a glibc-linked binary refuses to start on any distribution
older than the machine that built it — so the portable build is the default
and the dynamic one is kept for anyone who wants the system allocator and
resolver.

Each archive contains exactly one root entry:

- `omakure` on Linux/macOS
- `omakure.exe` on Windows

CI runs `cargo test --all-targets --locked`, release builds, and archive-content
assertions. `tests/packaging_smoke.rs` verifies the source/package contract
without requiring Docker.

The runtime workspace is created or mounted separately. See `deployment.md`
for the node-service container and `workspace.md` for its volume layout.
