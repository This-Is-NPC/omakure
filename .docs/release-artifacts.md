# Release artifact format

The release workflow builds one headless `omakure` executable per supported
platform. It does not package workspace scripts, themes, TUI assets, or widget
files.

## Archives

- `omakure-vX.Y.Z-linux-x86_64.tar.gz`
- `omakure-vX.Y.Z-darwin-x86_64.tar.gz`
- `omakure-vX.Y.Z-windows-x86_64.zip`

Each archive contains exactly one root entry:

- `omakure` on Linux/macOS
- `omakure.exe` on Windows

CI runs `cargo test --all-targets --locked`, release builds, and archive-content
assertions. `tests/packaging_smoke.rs` verifies the source/package contract
without requiring Docker.

The runtime workspace is created or mounted separately. See `deployment.md`
for the engine container and `workspace.md` for its volume layout.
