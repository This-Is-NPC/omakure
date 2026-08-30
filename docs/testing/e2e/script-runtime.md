# Script Runtime

**Status:** CI for shipped runtime coverage; optional interpreters remain platform-dependent.

## Source

- `tests/lua_script_kind_e2e.rs`
- Shell/runtime tests in the normal Rust test targets
- Runtime construction: `src/runtime.rs`

## Run

```bash
cargo test --test lua_script_kind_e2e --locked
cargo test --all-targets --locked
```

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

## Environment and Cleanup

Lua tests use temporary script directories and bounded child execution. The
packaged-image no-system-Lua proof is in [Docker image smoke](docker-image-smoke.md).

## Troubleshooting

- Lua host failure: check `Cargo.toml` for `mlua` with `vendored`; do not install system Lua as a workaround for the packaging contract.
- PowerShell/Python unavailable: treat it as the documented optional-runtime result unless the specific test requires that interpreter.
