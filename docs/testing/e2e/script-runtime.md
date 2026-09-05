# Script Runtime

**Status:** CI for shipped runtime coverage; optional interpreters remain
platform-dependent.

## Source

- `tests/lua_script_kind_e2e.rs`
- Shell/runtime tests in the normal Rust test targets
- Runtime construction: `src/runtime.rs`
- Canonical execution layers: `scripts/tasks/atomic/` and
  `scripts/tasks/suite/`

## Run

Use the canonical suite routes rather than embedding direct test commands:

```bash
mise run test:integration
mise run test:e2e
mise run test:node-service
```

For the complete local Linux gate, use `mise run check:full`. The platform
matrix uses `scripts/tasks/check/platform/{linux-gnu,linux-musl,macos,windows}`;
the selected suite owns native tests, target builds, static-link checks where
applicable, and binary smoke. A local Linux host is the only host with the
complete full scope.

## Proves

- Embedded Lua scripts run, capture stdout, receive hyphen-prefixed arguments byte-for-byte, and see the script path as argument zero.
- Lua runtime errors and child exit codes remain observable through the CLI.
- Missing scripts use the reserved host-failure code.
- A per-job timeout terminates a running Lua script.
- The host does not require a system Lua installation.
- Existing Bash, PowerShell, and Python command construction follows the supported runtime boundary; PowerShell and Python availability is reported rather than silently assumed.

## Does Not Prove

- Optional PowerShell/Python interpreters are not installed or exercised on every platform.
- It does not certify arbitrary scripts, shell quoting, or external commands beyond the fixtures.
- Hosted macOS and Windows jobs do not claim the Linux Docker certification.

## Environment and Cleanup

Lua tests use temporary script directories and bounded child execution. The
packaged-image no-system-Lua proof is in [Docker image smoke](docker-image-smoke.md).
Linux musl runners require `musl-tools` and `musl-gcc`; macOS runners require
an owned physical `RUNNER_TEMP`; Windows runners require the supported MSVC
target and static CRT setup. Manual Fedora VM/KVM execution is excluded from
these routes and must be requested explicitly with `mise run cert:vm`.

## Troubleshooting

- Lua host failure: check `Cargo.toml` for `mlua` with `vendored`; do not install system Lua as a workaround for the packaging contract.
- PowerShell/Python unavailable: treat it as the documented optional-runtime result unless the specific test requires that interpreter.
