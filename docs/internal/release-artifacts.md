# Release artifact format

The release workflow builds one headless `omakure` executable for each supported
target. It packages nothing else: no workspace scripts, no assets.

## Archives

| CI platform | Rust target | Release asset |
|---|---|---|
| Linux x86_64 | `x86_64-unknown-linux-gnu` | `omakure-vX.Y.Z-linux-x86_64.tar.gz` |
| Linux x86_64, musl | `x86_64-unknown-linux-musl` | `omakure-vX.Y.Z-linux-musl-x86_64.tar.gz` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` | `omakure-vX.Y.Z-linux-aarch64.tar.gz` |
| Linux aarch64, musl | `aarch64-unknown-linux-musl` | `omakure-vX.Y.Z-linux-musl-aarch64.tar.gz` |
| macOS x86_64 | `x86_64-apple-darwin` | `omakure-vX.Y.Z-darwin-x86_64.tar.gz` |
| macOS aarch64 | `aarch64-apple-darwin` | `omakure-vX.Y.Z-darwin-aarch64.tar.gz` |
| Windows x86_64 | `x86_64-pc-windows-msvc` | `omakure-vX.Y.Z-windows-x86_64.zip` |
| Windows aarch64 | `aarch64-pc-windows-msvc` | `omakure-vX.Y.Z-windows-aarch64.zip` |

`scripts/install/install.sh` selects the host architecture and prefers the matching `linux-musl`
archive, falling back to the matching glibc archive when a release predates it.
The PowerShell installer selects the matching Windows `x86_64` or `aarch64`
archive. Omakure is installed on machines the operator does not control, and a
glibc-linked binary refuses to start on any distribution older than the machine
that built it, so the portable build is the default and the dynamic one is kept
for anyone who wants the system allocator and resolver.

Each archive contains exactly one root entry:

- `omakure` on Linux/macOS
- `omakure.exe` on Windows

## CI evidence

The release workflow has eight matrix entries, one for each target above. Each
entry runs `cargo test --all-targets --locked`, builds its target, runs the
resulting release binary's `--version` smoke check on that CI runner, and
asserts that its archive contains only the expected binary. The musl entries
also assert static linking. The matrix and archive checks are CI evidence for
those target builds and package contents; they do not claim that a release
binary was executed locally on every supported platform.

`tests/packaging_smoke.rs` verifies the source/package contract without requiring
Docker. The Linux Docker smoke and certification jobs are separate CI evidence;
they do not turn the packaging test into a local service or installer test.

The runtime workspace is created or mounted separately. See `../deployment.md`
for the node-service container and `../workspace.md` for its volume layout.
