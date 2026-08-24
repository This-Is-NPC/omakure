# Headless Contract Baseline

This is the pre-removal safety net for task #2673. It freezes the public
headless contract before the destructive cleanup. The tables below are a
contract inventory, not a proposal to remove anything listed as retained.

## Baseline Context

Measured from commit `27f5dd970ffdf3003859c0394f719f78d6163a86` on the
`rebuilding-omakure-for-omarchy` branch.

| Item | Value |
|---|---|
| Package | `omakure 0.2.0` |
| Host/target | `x86_64-unknown-linux-gnu` |
| Rust | `rustc 1.97.1 (8bab26f4f 2026-07-14)` |
| Profile | Cargo default `release` profile (`cargo build --release`) |
| Release binary | `target/release/omakure`: `10,520,464` bytes |
| Installer binary | `target/release/omakure-installer`: `442,560` bytes |
| Normal direct dependencies | 27 |

Reproduce the binary measurements from a clean checkout with:

```bash
cargo build --release
stat -c '%n %s bytes' target/release/omakure target/release/omakure-installer
```

Reproduce the normal dependency inventory with:

```bash
cargo tree --edges normal --depth 1
```

The direct normal dependency inventory at this baseline is:

```text
argon2 0.5.3       axum 0.7.9           chrono 0.4.44
clap 4.5.56        clap_complete 4.5.65 cron 0.12.1
crossterm 0.27.0   daemonize 0.5.0      dirs 5.0.1
fs2 0.4.3          humantime 2.3.0      libc 0.2.178
mlua 0.9.9         rand 0.8.6           ratatui 0.26.3
rattles 0.2.2      rusqlite 0.31.0      serde 1.0.228
serde_json 1.0.148 serde_urlencoded 0.7.1 sha2 0.10.9
signal-hook 0.3.18 subtle 2.6.1       thiserror 1.0.69
tokio 1.50.0       toml 0.8.23          tower 0.5.3
```

The target, profile, commands, package version, and commit are part of the
measurement. A post-removal comparison must use the same target and profile;
dependency lockfile changes must be reported separately from code removal.

## CLI Surface

Global retained options are `--scripts-dir`, `--json`, and the retained
machine-readable envelope behavior. The positional `PATH` mode is intentionally
removed with the TUI; it must not be reintroduced as a headless alias.

| Top-level command | Decision | Contract and executable coverage |
|---|---|---|
| `run` | Retain | Inline execution, actor/reason/run IDs, env-file and secret resolution, redacted history; `tests/secret_cli_e2e.rs`, `src/cli/run.rs` tests |
| `doctor` / `check` | Retain | Runtime, workspace, and schema diagnostics; `tests/cli_surface_e2e.rs` |
| `scripts` | Retain | Recursive catalogue and AND tag filtering; `tests/cli_surface_e2e.rs`, `tests/cli_positional_path.rs` |
| `describe` | Retain | One script's parsed schema and redacted secret metadata; `tests/cli_surface_e2e.rs`, `src/cli/describe.rs` tests |
| `search` | Retain | SQLite index query and tag filtering; `tests/cli_surface_e2e.rs` and operation tests |
| `history` | Retain | `list`, `show`, `tail`, `stats`, and `traces`; `tests/cli_surface_e2e.rs`, `tests/secret_cli_e2e.rs` |
| `queue` | Retain | `add`, `cancel`, `dead-letter`, `worker`, and `stats`; `tests/cli_surface_e2e.rs`, `tests/secret_cli_e2e.rs` |
| `battery` | Retain | `list`, `add`, `sync`, `inspect`, `scripts`, `install`, and `remove`; local repository lifecycle in `tests/cli_surface_e2e.rs` and `tests/cli_battery.rs` |
| `token` | Retain | `generate`, Argon2id hashes, scoped entries, and confirmed append; `tests/cli_surface_e2e.rs`, `tests/http_api_e2e.rs` |
| `api` | Retain | Loopback HTTP management API, auth, scopes, and routes below; `tests/http_api_e2e.rs` |
| `engine` | Retain | API plus optional workers/scheduler and readiness; `tests/engine_e2e.rs`, `tests/policy_e2e.rs` |
| `trace` | Retain | Child-script structured trace insertion with redaction; `tests/cli_surface_e2e.rs`, `src/cli/trace.rs` tests |
| `help-ai` | Retain | Clap-derived AI capability discovery; `tests/cli_surface_e2e.rs`, `src/cli/help_ai.rs` tests |
| `init` | Retain | Schema-aware script generation, stdin body, force behavior; `tests/cli_surface_e2e.rs`, `src/cli/init.rs` tests |
| `env` | Retain | `list`, `create`, `show`, `set`, `remove`, `replace`, `activate`, `deactivate`, `delete`; `tests/secret_cli_e2e.rs`, `src/cli/env.rs` tests |
| `config` | Retain | Resolved paths and masked environment diagnostics, including JSON; `tests/cli_surface_e2e.rs`, `tests/cli_positional_path.rs` |
| `update` | Retain | Release self-update and missing-script sync; unit tests in `src/cli/update.rs`; network and in-place replacement are not run in CI |
| `uninstall` | Retain | Binary removal and optional workspace wipe; unit tests in `src/cli/uninstall.rs`; destructive host mutation is not run in CI |
| `completion` | Retain | Bash, Zsh, Fish, and Pwsh generation; black-box Bash generation in `tests/cli_surface_e2e.rs` |
| `theme` | Remove | Theme CLI and theme configuration are outside the headless product; current parser and tests are removal inputs only |
| no command / `PATH` | Remove | TUI launch, positional TUI path mode, and session-only scripts-root behavior are outside the headless product |

Nested command names are also frozen: `history list|show|tail|stats|traces`,
`queue add|cancel|dead-letter|worker|stats`,
`battery list|add|sync|inspect|scripts|install|remove`,
`env list|create|show|set|remove|replace|activate|deactivate|delete`, and
`token generate`. Clap command-set drift is checked by
`command_surface_inventory_maps_all_current_commands` in
`tests/cli_surface_e2e.rs`.

## JSON Contract

Supported `--json` commands emit exactly one stdout envelope:

```json
{
  "ok": true,
  "data": {},
  "error": null,
  "schema_version": "1"
}
```

Failures set `ok` to `false`, `data` to `null`, and provide
`error.code` plus `error.message`. `help-ai` always emits this envelope.
The stable CLI error codes are `not_found`, `schema_invalid`, `script_exists`,
`missing_required_field`, `invalid_argument`, `not_implemented`, `internal`,
`daemon_already_running`, and `daemon_not_running`. The helper and code list
are centralized in `src/cli/json.rs`; changing a code or the envelope shape is
a breaking contract change.

HTTP uses the same envelope and schema version. Its stable status mapping is:
401 unauthorized, 403 forbidden/policy denial, 400 invalid input, 404 not
found, 409 conflict or invalid transition, 413 oversized payload, 415
unsupported script/content, 501 unsupported operation, and 500 internal error.
Operation-specific codes such as `not_synced`, `manifest_invalid`,
`unsafe_path`, `git_failed`, and `registry_invalid` remain stable at the
operation boundary.

## HTTP Routes, Auth, and Scopes

The API binds to `127.0.0.1:7878` by default. Non-loopback binding requires
`--allow-non-loopback`. Every route except health and readiness requires a
Bearer token. Preferred tokens-file mode authenticates Argon2id hashes and
uses per-token scopes; legacy mode uses `OMAKURE_API_TOKEN` plus repeated
`--capability` flags. Missing/invalid tokens return 401; an authenticated token
without the required scope returns 403. Request bodies are limited to 1 MiB.

| Method | Route | Auth / scope |
|---|---|---|
| `GET` | `/v1/health` | None |
| `GET` | `/v1/ready` | None; minimal readiness payload |
| `GET` | `/v1/admin/status` | `admin:status` |
| `GET` | `/v1/config` | `config:read` |
| `GET` | `/v1/doctor` | `config:read` / `doctor:read` |
| `GET` | `/v1/workspace` | `config:read` / `workspace:read` |
| `GET` | `/v1/search` | `scripts:read` / `search:read` |
| `GET` | `/v1/tree` | `scripts:read` |
| `GET` | `/v1/tree/*path` | `scripts:read` |
| `GET` | `/v1/scripts` | `scripts:read` |
| `GET` | `/v1/scripts/*script_id` | `scripts:read` |
| `GET` | `/v1/scripts/*script_id/schema` | `scripts:read` |
| `GET` | `/v1/scripts/*script_id/content` | `scripts:read` |
| `GET` | `/v1/envs` | `env:read` / `envs:read` |
| `POST` | `/v1/envs` | `env:write` / `envs:write` |
| `GET` | `/v1/envs/:name` | `env:read` / `envs:read` |
| `PUT` | `/v1/envs/:name` | `env:write` / `envs:write` |
| `PATCH` | `/v1/envs/:name` | `env:write` / `envs:write` |
| `DELETE` | `/v1/envs/:name` | `env:write` / `envs:write` |
| `POST` | `/v1/envs/:name/activate` | `env:activate` / `envs:activate` |
| `DELETE` | `/v1/envs/active` | `env:activate` / `envs:activate` |
| `PUT` | `/v1/envs/:name/params/:key` | `env:write` / `envs:write` |
| `DELETE` | `/v1/envs/:name/params/:key` | `env:write` / `envs:write` |
| `GET` | `/v1/runs` | `runs:read` |
| `POST` | `/v1/runs` | `runs:write` / `runs:enqueue` |
| `GET` | `/v1/runs/:run_id` | `runs:read` |
| `GET` | `/v1/runs/:run_id/traces` | `runs:read` |
| `POST` | `/v1/runs/:run_id/cancel` | `runs:write` / `runs:cancel` |
| `POST` | `/v1/runs/:run_id/dead-letter` | `runs:write` / `runs:dead-letter` |
| `GET` | `/v1/queue/stats` | `runs:read` |
| `GET` | `/v1/batteries` | `batteries:read` |
| `POST` | `/v1/batteries` | `batteries:write` / `batteries:add` |
| `GET` | `/v1/batteries/:battery_id` | `batteries:read` |
| `DELETE` | `/v1/batteries/:battery_id` | `batteries:write` / `batteries:remove` |
| `GET` | `/v1/batteries/:battery_id/scripts` | `batteries:read` |
| `POST` | `/v1/batteries/:battery_id/scripts/:script_id/install` | `batteries:write` / `batteries:install` |
| `POST` | `/v1/batteries/:battery_id/sync` | `batteries:write` / `batteries:sync` |
| `GET` | `/v1/secrets` | `secrets:read-metadata` and policy opt-in |

The route inventory is checked against the router by
`http_route_inventory_maps_all_current_router_entries` in
`tests/http_api_e2e.rs` and by the in-crate API inventory tests. Secret values,
token plaintext, token hashes, local paths in readiness, and authorization
headers must never appear in responses or audit logs.

## Engine, Queue, and State

`omakure engine` retains the API surface and can embed the queue worker and
scheduler. `--workers 0 --no-scheduler` is API-only. The default is one worker
and the scheduler enabled; readiness flags can require configured loops to be
alive. SIGTERM/SIGINT shuts down HTTP first, then scheduler/claiming, then
workers. Engine behavior is covered by `tests/engine_e2e.rs` and policy behavior
by `tests/policy_e2e.rs`.

The SQLite run state machine in `src/runs.rs` retains these states and
transitions:

| From | Allowed destination |
|---|---|
| `queued` | `running`, `cancelled` |
| `running` | `completed`, `failed`, `cancelled`, `timed_out` |
| `failed` | `dead_letter` |
| `timed_out` | `dead_letter` |
| terminal states | No further transition |

Workers claim atomically, heartbeat leases, honor actor/script filters, and
respect cancellation and timeout. `run`, queue workers, and scheduled runs all
use `run_executor::execute_with_heartbeat`. History is private SQLite storage;
the supported read verbs are `history list|show|stats|traces` and `queue stats`.
Trace rows use monotonic per-run sequence numbers and support level and
`since-sequence` filtering.

`serve` retains the five-second schedule scan, cron normalization, the
`.omakure/daemon.pid` lock, `.omakure/daemon.log`, one-shot test mode, optional
in-process worker, and overlap prevention by `cron_schedule_id`. Systemd
install/uninstall/status are retained host integrations; only safe status and
unit-rendering tests run in the normal suite.

## Environments and Secrets

The workspace environment contract retains named `.conf` environments under
`.omakure/envs/`, the active-environment marker, CLI lifecycle operations, HTTP
environment routes, per-run `--env-file`, precedence of active env then
per-run env then reserved Omakure variables, and masking of sensitive values.

Secret fields use `Type: "secret"`. Supported provider references include
`secret://env/NAME`, legacy `secret://env:NAME`, and
`secret://provider/key`. Plaintext secret values are passed only to the child
process, while persisted arguments and captured output are redacted. Queued
HTTP secret inputs must be reconstructable references. `/v1/secrets` is
metadata-only and requires both `secrets:read-metadata` and the policy opt-in.

## Batteries, Workspace, and Schema

Batteries retain registry, HTTPS/private-token validation, detached sync,
manifest inspection, safe script listing, trusted-workspace installation with
provenance, force overwrite, and cache removal. Cached repositories are
untrusted and are never executed directly. CLI local sources remain supported;
HTTP registration is HTTPS-only.

The global workspace retains `.omakure/` metadata, `.history/runs.sqlite`, the
SQLite search index, `omakure.toml`, named environments, ignore files, and the
configured scripts root. Supported script extensions are `.bash`, `.sh`,
`.ps1`, and `.py`, using Bash, PowerShell, and Python runtime resolution.

Scripts retain the schema block between `OMAKURE_SCHEMA_START` and
`OMAKURE_SCHEMA_END`, extension-appropriate comment prefixes, PascalCase schema
keys (`Name`, `Description`, `Fields`, `Schedule`), field validation and
defaults, and cron schedule parsing. Path confinement rejects traversal,
absolute paths outside the workspace, symlink escapes, metadata paths, and
unsupported content.

## Intentionally Removed Surface

The following is explicitly outside the headless contract and is input to the
next destructive task, not an implementation change in this task:

| Surface | Removal boundary |
|---|---|
| Ratatui/crossterm TUI | `src/adapters/tui/**`, TUI app/events/render/state/widgets |
| TUI startup | no-argument launch and all TUI screen navigation |
| Positional TUI path mode | `omakure PATH`, session-only scripts root, and its path-specific behavior |
| Themes | `theme` CLI, `theme_config`, built-in `themes/*.toml`, Omarchy theme import, and theme-only spinner styling |
| Lua directory widgets | `lua_widget`, directory `index.lua` loading, and widget-specific documentation |

No TUI production code is removed by task #2673. The current TUI/theme/Lua
tests remain baseline evidence until the deletion task updates the dependency
and test surface deliberately.

## Verification Snapshot

The pre-removal gates completed on this checkout:

```text
cargo test: 1,192 passed, 0 failed
cargo clippy --all-targets -- -D warnings: passed
cargo fmt --check: passed after formatting-only normalization in
  src/operations/battery.rs
cargo build --release: passed
```

`src/auth.rs` also received a test-only fixture optimization: the legacy token
compatibility test keeps a non-target record but no longer spends production
Argon2 cost on 63 redundant hashes. It still exercises the legacy fallback and
wrong-token rejection; maximum-file selector coverage remains in the selector
tests. This is required to make the full baseline suite reproducible on slower
hosts and does not change production authentication behavior.
