# Development

Omakure is built and exercised as a headless CLI/HTTP application. Do not use
bare `omakure` as an application launch command: no-argument invocation only
prints help and returns; development entry points must name a command.

## Fast path

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
mise run dev
```

`mise run dev` builds the debug binary, starts a node service on a disposable local
port, verifies `/v1/health` and `/v1/ready`, then terminates it. It does not
leave a daemon running or require a terminal UI. Set `OMAKURE_DEV_WORKSPACE`
and `OMAKURE_DEV_PORT` to override its fixtures.

## Mise tasks

| Task | Purpose |
|---|---|
| `mise run build` | `cargo build` |
| `mise run test` | `cargo test` |
| `mise run lint` | clippy with warnings denied and `cargo fmt --check` |
| `mise run dev` | bounded node-service health/readiness smoke check |
| `mise run node` | run the node service in the foreground |
| `mise run node-service-check` | focused CLI/HTTP/node-service integration tests |
| `mise run coverage` | tarpaulin coverage report |
| `mise run install` | `cargo install --path .` |

The repository `scripts/` directory is a fixture workspace for debug builds.
The global `--scripts-dir` flag and `OMAKURE_SCRIPTS_DIR` override it. Repo
helpers live in `.scripts/`; they must use explicit CLI commands and bounded
process cleanup.

## Architecture guide

- `src/domain/`: pure schema parsing, validation, and cron logic.
- `src/operations/`: shared behavior called by CLI and HTTP adapters.
- `src/cli/`: clap commands, JSON envelopes, API/node-service lifecycle, workers,
  scheduler, history, and local lifecycle commands.
- `src/runs.rs`: SQLite run state machine and trace storage.
- `src/run_executor.rs`: one execution path for direct, queued, and scheduled runs.
- `src/adapters/`: filesystem repository, process runner, environments, and
  runtime checks.
- `src/auth.rs`, `src/policy.rs`, `src/secrets.rs`, `src/redaction.rs`: deploy
  trust boundaries and secret handling.

The HTTP layer must remain an adapter. Add shared validation or behavior to an
operation, then cover it at the CLI/HTTP boundary as appropriate. Do not open
SQLite from route handlers or duplicate CLI logic in HTTP handlers.

## Focused tests

```bash
cargo test --test cli_surface_e2e
cargo test --test node_service_e2e
cargo test --test http_api_e2e
cargo test --test policy_e2e
cargo test --test packaging_smoke
```

The integration suites launch the compiled binary, use temporary workspaces,
exercise real SQLite state, and test HTTP readiness/authentication. Keep
secrets out of assertions and logs. Use inline unit tests for pure parsers,
operations, state transitions, redaction, and runtime resolution.

## Release checks

CI runs `cargo test --all-targets --locked`, release builds for Linux/macOS/
Windows, clippy, formatting, packaging assertions, and release-note/version
validation. Before changing a command contract, run `omakure help-ai` from the
built binary and update `.docs/ai-interface.md`, `.docs/cli-http-parity.md`,
and the relevant tests.
