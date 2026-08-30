# CLI Surface

**Status:** CI on all supported runners; Windows serve-stop cases are Windows CI-specific.

## Source

- `tests/cli_surface_e2e.rs`
- `tests/secret_cli_e2e.rs` and `tests/cli_battery.rs` cover adjacent command behavior.

## Run

```bash
cargo test --test cli_surface_e2e --locked
cargo test --test secret_cli_e2e --test cli_battery --locked
```

The suite's `command_surface_inventory_maps_all_current_commands` test compares
the hand-maintained inventory with Clap help output. This is a drift tripwire,
not proof that every inventory pointer invokes the command.

## Proves

- JSON envelopes and representative local commands: `init`, `describe`, `search`, `doctor`, `help-ai`, `config`, token generation, completion, and `serve --once`.
- `serve --once` writes both lifecycle log entries; `serve --status` is host-safe.
- Environment, history, queue, battery, node, token, and nested command names remain present in the public help surface.
- Node initialization, status, trust, revoke, baseline refusal without a service, and confirmed reset behavior are exercised through the binary.

## Does Not Prove

- `uninstall`, `update`, and daemon/systemd mutation are only help-surface or safe-boundary checks.
- `battery install` is Unix-only; it is not a cross-platform guarantee.
- Windows `serve --stop` tests run only on Windows and do not represent Unix daemon behavior.

## Environment and Cleanup

Tests use temporary workspaces and bounded child-process helpers. Windows stop
tests poll for the PID file and process exit for 10 seconds and preserve an
indeterminate PID file rather than killing an unrelated process.

## Troubleshooting

- Inventory mismatch: inspect `omakure --help` and `src/cli/args.rs`; add behavioral coverage separately from the inventory row.
- `serve --once` failure: inspect the temporary `.omakure/daemon.log`, not a global service unit.
