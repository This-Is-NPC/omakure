# Implemented Requirements

This inventory describes the compiled headless baseline. Source paths are
references, not design proposals; if a command or module is removed, this file
must be updated in the same change.

## Functional requirements

| ID | Requirement | Source |
|---|---|---|
| FR-001 | No-argument invocation prints CLI help; JSON mode returns one `invalid_argument` envelope. | `src/main.rs`, `tests/cli_no_subcommand.rs` |
| FR-002 | Global `--scripts-dir` and environment overrides resolve one workspace root; positional paths are rejected. | `src/main.rs`, `src/cli/args.rs`, `tests/cli_surface_e2e.rs` |
| FR-003 | Recursive script listing supports `.bash`, `.sh`, `.ps1`, and `.py`, nested `.omakureignore`, and repeatable AND tag filters. | `src/adapters/workspace_repository.rs`, `src/runtime.rs`, `src/cli/list.rs` |
| FR-004 | Embedded PascalCase schemas parse and validate fields, outputs, queue declarations, secret fields, and schedules. | `src/domain/schema.rs`, `src/domain/parsing.rs` |
| FR-005 | `describe` returns a complete parsed schema and resolved path; malformed schemas and missing scripts have stable errors. | `src/cli/describe.rs`, `src/operations/core.rs` |
| FR-006 | SQLite full-text search supports CLI queries and repeatable tag filters without rebuilding per request. | `src/search_index.rs`, `src/cli/search.rs`, `src/operations/search.rs` |
| FR-007 | `init` creates schema-bearing Bash, PowerShell, or Python templates, validates supplied schema JSON, reads optional body stdin, and protects existing files unless forced. | `src/cli/init.rs` |
| FR-008 | Direct runs support actors, reasons, caller IDs, parent IDs, forwarded args, per-run env files, no-prompt mode, timeouts, and secret inputs. | `src/cli/run.rs`, `src/run_executor.rs` |
| FR-009 | Bash, PowerShell, and Python commands resolve interpreters and preserve injected `PATH` semantics. | `src/runtime.rs`, `src/adapters/script_runner.rs` |
| FR-010 | Active and per-run environment values are injected with reserved Omakure variables last; sensitive values are masked and not persisted. | `src/adapters/environments.rs`, `src/run_executor.rs`, `src/redaction.rs` |
| FR-011 | Named environments can be listed, created, shown, set, removed, replaced, activated, deactivated, and deleted through CLI and HTTP. | `src/cli/env.rs`, `src/operations/envs.rs` |
| FR-012 | Runs are stored in SQLite with state, actor, reason, args, output, timing, trigger, and schedule provenance. | `src/runs.rs` |
| FR-013 | Queue producers add, cancel, dead-letter, and report jobs; workers claim jobs atomically, heartbeat leases, honor timeouts, and drain on signals. | `src/cli/queue.rs`, `src/runs.rs`, `src/run_executor.rs` |
| FR-014 | History lists, shows, tails, aggregates, and filters runs; trace events can be written from a child and read incrementally. | `src/cli/history.rs`, `src/cli/trace.rs`, `src/runs.rs` |
| FR-015 | Schema schedules accept supported cron forms, enqueue due runs every five seconds, skip overlap, and log lifecycle/errors. | `src/domain/schedule.rs`, `src/cli/serve.rs` |
| FR-016 | Linux systemd user lifecycle operations install, uninstall, and report the per-workspace scheduler service. | `src/cli/serve_autostart.rs`, `src/cli/serve.rs` |
| FR-017 | Batteries can be registered, synced, inspected, listed, installed with validation/provenance, and removed; cached content is untrusted. | `src/cli/battery.rs`, `src/operations/battery.rs` |
| FR-018 | `doctor`, `config`, `completion`, `update`, and `uninstall` provide local diagnostics, integration, lifecycle, and release operations. | `src/cli/doctor.rs`, `src/cli/config.rs`, `src/main.rs` |
| FR-019 | `help-ai` derives a machine-readable command and data-shape inventory from clap metadata. | `src/cli/help_ai.rs`, `src/cli/args.rs` |
| FR-020 | CLI JSON uses `{ ok, data, error, schema_version }` and stable error codes. | `src/cli/json.rs`, `src/cli/args.rs` |
| FR-021 | `api` exposes authenticated management routes for config, diagnostics, workspace, scripts, search, runs, queues, environments, Batteries, and secret metadata. | `src/cli/api.rs`, `src/operations/*.rs` |
| FR-022 | `node serve` validates machine state, initializes one identity and empty trust registry when absent, then composes HTTP, optional workers, and optional scheduler with coordinated shutdown and readiness gates. | `src/cli/node_service.rs`, `src/operations/node.rs`, `src/cli/args.rs` |
| FR-023 | Health and readiness are unauthenticated; other HTTP routes require scoped bearer tokens or explicitly granted legacy capabilities. | `src/auth.rs`, `src/cli/api.rs`, `src/cli/node_service.rs` |
| FR-024 | Deploy policy controls route groups, auth modes, body limits, script limits, environment use, secret use, and node-service scheduler/worker defaults. | `src/policy.rs`, `src/cli/api.rs`, `src/cli/node_service.rs` |
| FR-025 | Direct transport provides authenticated encrypted sessions with bounded framing, static peer validation, trust authorization, replay protection, revocation handling, and redacted audit outcomes. | `src/direct_transport.rs`, `src/direct_service.rs`, `src/node_transport.rs` |
| FR-026 | LAN discovery is bounded and trust-neutral; manual enrollment and signed enrollment bundles validate identity binding, audience, expiry, replay, authority, and revocation before trust mutation. | `src/discovery.rs`, `src/enrollment.rs`, `src/node_registry.rs` |

## Non-functional requirements

| ID | Requirement | Source |
|---|---|---|
| NFR-001 | Linux, macOS, and Windows builds use conditional platform adapters for paths, daemonization, signals, and services. | `src/main.rs`, `src/cli/serve.rs`, `src/cli/serve_autostart.rs`, `Cargo.toml` |
| NFR-002 | CLI and HTTP behavior remains protocol-neutral in `operations/`; adapters do not duplicate business rules. | `src/operations/`, `src/cli/`, `src/cli/api.rs` |
| NFR-003 | SQLite uses WAL/busy-timeout behavior for concurrent local readers and writers; one workspace remains single-host storage. | `src/runs.rs`, `src/search_index.rs` |
| NFR-004 | HTTP request bodies and script/tree responses are bounded, and unsafe paths/symlinks/metadata paths are rejected. | `src/cli/api.rs`, `src/operations/scripts.rs` |
| NFR-005 | Bearer tokens are hashed, scopes are explicit, token values are redacted from logs/responses, and auth failures do not reveal secrets. | `src/auth.rs`, `src/cli/api.rs` |
| NFR-006 | Release CI tests all targets, denies clippy warnings, checks formatting, verifies binary-only archives, and requires matching release notes. | `.github/workflows/ci.yml`, `.github/workflows/release.yml`, `tests/packaging_smoke.rs` |
| NFR-007 | The shipped package contains no TUI/theme/widget code or removed direct dependencies. | `tests/packaging_smoke.rs`, `Cargo.toml` |
| NFR-008 | Linux CI runs the bounded four-service transport certification; Linux, macOS, and Windows CI run native protocol/build/lifecycle coverage without Docker assumptions. | `.scripts/transport-certification.sh`, `.github/workflows/ci.yml` |

## Business rules

| ID | Rule | Source |
|---|---|---|
| BR-001 | `.history`, `.git`, and `.omakure` metadata are excluded from script discovery. | `src/adapters/workspace_repository.rs` |
| BR-002 | Schema markers and field names use extension comment syntax and PascalCase JSON keys. | `src/domain/parsing.rs`, `src/domain/schema.rs` |
| BR-003 | Scheduled runs use declared field defaults; missing defaults are omitted rather than blocking the scheduler. | `src/cli/serve.rs` |
| BR-004 | Scheduler overlap is keyed by canonical script path and cron expression. | `src/cli/serve.rs`, `src/runs.rs` |
| BR-005 | Secret schema fields cannot declare choices; plaintext secret values are redacted while provider references can be retained. | `src/domain/schema.rs`, `src/secrets.rs`, `src/runs.rs` |
| BR-006 | Omakure-reserved `OMAKURE_RUN_ID` and `OMAKURE_SCRIPTS_DIR` values cannot be overridden by managed or per-run environments. | `src/run_executor.rs` |
| BR-007 | HTTP Battery registration is HTTPS-only and cached repositories are never executed directly. | `src/operations/battery.rs`, `src/cli/api.rs` |
| BR-008 | Non-loopback HTTP binding requires explicit opt-in and route policy cannot be bypassed by token scope. | `src/cli/api.rs`, `src/policy.rs` |
| BR-009 | No positional script path, TUI launch, theme configuration/assets, or directory `index.lua` widget behavior is part of the current product contract. | `src/cli/args.rs`, `src/main.rs`, `tests/packaging_smoke.rs` |
| BR-010 | Machine identity and trust are independent of script workspaces; normal update, replacement, restart, and uninstall preserve node state, while `node reset --confirmed` removes it and creates no replacement until the next service start. | `src/node.rs`, `src/node_identity.rs`, `src/operations/node.rs`, `src/cli/node.rs` |
| BR-011 | Direct transport never grants trust or authorization by handshake alone; only explicit enrollment/trust operations may mutate active peer state, and malformed, oversized, downgraded, spoofed, wrong-target, replayed, expired, or revoked inputs fail closed. | `src/direct_service.rs`, `src/node_registry.rs`, `tests/direct_transport_contract.rs` |
