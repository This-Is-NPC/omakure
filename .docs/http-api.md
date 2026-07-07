# HTTP API Contract

This is the v1 contract for Omakure's internal HTTP management API. It is
the implementation source for the `http-management-api` plan and reconciles
the older `.temp/plan.md` brief with the current codebase: Omaken is gone,
and Battery CLI/operations already exist.

## Goal

Expose Omakure through a small internal HTTP API in the same binary so a
trusted local process, sidecar, or internal-network container can manage a
workspace without shelling out to the CLI.

HTTP is an adapter. It must not own business logic. Each route translates an
HTTP request into a shared operation request, calls `src/operations/*`, then
serializes the operation result.

## Non-Goals

- Public internet API.
- Browser-facing CORS support.
- OAuth, OIDC, sessions, users, or RBAC.
- Distributed queue across hosts.
- Shared SQLite over network filesystems.
- Direct execution of Battery scripts from Git cache.
- A second product surface with behavior that differs from CLI operations.

## Framework Decision

Use `axum` with `tokio` for v1.

Why:

- Middleware and extractors keep auth, body limits, and route state testable.
- `tower` service testing lets endpoint tests run without binding real ports.
- The async runtime is contained behind `omakure api`; existing CLI/TUI code
  remains synchronous unless an operation already needs async in a later task.

## Command Contract

```bash
omakure api --bind 127.0.0.1:7878
omakure api --bind 0.0.0.0:7878 --allow-non-loopback
```

Defaults and guards:

- Default bind: `127.0.0.1:7878`.
- Loopback bind is allowed without extra flags.
- Non-loopback bind requires `--allow-non-loopback`.
- `0.0.0.0` and `::` count as non-loopback.
- Binding must fail before listening when the guard is not satisfied.

## Authentication Contract

Every endpoint except health requires:

```http
Authorization: Bearer <token>
```

Token source:

- `OMAKURE_API_TOKEN`

Rules:

- Reject missing token configuration before serving protected endpoints.
- Reject an empty token.
- Reject known default/example tokens such as `changeme`, `password`,
  `secret`, `token`, and `omakure`.
- Require a minimum token length of 32 bytes.
- Compare presented and configured tokens in constant time where practical.
- Redact token values from logs, errors, JSON responses, and tests fixtures.
- Never inject `OMAKURE_API_TOKEN` into script environments.
- CORS is disabled by default. Do not add permissive CORS in v1.

## Request Limits

Apply a request body limit before JSON extraction.

V1 limit: 1 MiB.

Rationale: the API accepts small management requests, not artifact uploads.

## JSON Envelope

HTTP responses use the same envelope shape as CLI JSON where practical:

```json
{
  "ok": true,
  "data": {},
  "error": null,
  "schema_version": "1"
}
```

Error response:

```json
{
  "ok": false,
  "data": null,
  "error": {
    "code": "not_found",
    "message": "resource was not found"
  },
  "schema_version": "1"
}
```

Error mapping:

| Error family | HTTP status |
|---|---:|
| auth missing/invalid | 401 |
| forbidden bind/config policy | 403 |
| invalid input / validation | 400 |
| not found | 404 |
| conflict / invalid transition / unsynced Battery | 409 |
| unsupported script/content media | 415 |
| payload too large | 413 |
| unsupported operation | 501 |
| internal I/O or unexpected error | 500 |

Operation-specific errors keep their stable operation code, such as
`not_synced`, `manifest_invalid`, `unsafe_path`, `git_failed`, or
`registry_invalid`.

The implemented v1 server uses the same envelope helper as CLI JSON output.
`schema_version` is a string for parity with CLI responses.

## Endpoint Contract

Health is unauthenticated:

```http
GET /v1/health
```

Read endpoints require auth:

```http
GET /v1/config
GET /v1/doctor
GET /v1/workspace
GET /v1/search
GET /v1/tree
GET /v1/tree/{path}
GET /v1/scripts
GET /v1/scripts/{script_id}
GET /v1/scripts/{script_id}/schema
GET /v1/scripts/{script_id}/content
GET /v1/runs
GET /v1/runs/{run_id}
GET /v1/runs/{run_id}/traces
GET /v1/queue/stats
GET /v1/batteries
GET /v1/batteries/{battery_id}
GET /v1/batteries/{battery_id}/scripts
```

Write endpoints require auth:

```http
POST /v1/runs
POST /v1/runs/{run_id}/cancel
POST /v1/runs/{run_id}/dead-letter
POST /v1/batteries
POST /v1/batteries/{battery_id}/sync
POST /v1/batteries/{battery_id}/scripts/{script_id}/install
DELETE /v1/batteries/{battery_id}
```

`POST /v1/runs` enqueues by default. It does not block on inline execution.

Write request bodies:

```json
POST /v1/runs
{
  "script": "tools/job",
  "args": ["--flag"],
  "run_id": "optional-caller-id",
  "actor": "agent",
  "reason": "why this was queued",
  "priority": 10,
  "timeout_ms": 60000,
  "parent_run_id": null,
  "cron_schedule_id": null
}
```

Defaults: `args=[]`, `actor="human"`, `priority=0`.

```json
POST /v1/runs/{run_id}/cancel
{ "reason": "optional" }

POST /v1/runs/{run_id}/dead-letter
{ "reason": "optional" }

POST /v1/batteries
{
  "name": "azure",
  "git_url": "https://example.invalid/azure.git",
  "requested_ref": "main"
}

POST /v1/batteries/{battery_id}/scripts/{script_id}/install
{ "force": false }
```

Battery defaults: `requested_ref="main"`, `force=false`, and
`DELETE /v1/batteries/{battery_id}` defaults to keeping the cache. Add
`?remove_cache=true` to delete the cached clone while unregistering.

HTTP Battery registration accepts `https://` Git URLs only. Local paths,
`file://`, and plaintext `http://` sources remain outside the HTTP API trust
boundary; use the local CLI for local development sources.

Read query parameters and safety policy:

- `GET /v1/config` returns the full config shape, but HTTP masks every active
  environment value. Plaintext env diagnostics are CLI-only.
- `GET /v1/search?q=<query>&tag=<tag>` searches scripts using the existing
  SQLite index. HTTP does not rebuild the index per request. `query` is accepted
  as an alias for `q`; repeated `tag` parameters are AND-filtered. Empty queries
  are rejected. Query length is capped at 256 bytes; tags are capped at 16 total
  and 64 bytes each.
- `GET /v1/tree/{path}` lists directories/scripts under the scripts root. It
  honors nested `.omakureignore` files through the shared workspace repository.
  Listings are capped at 1000 entries.
- `GET /v1/scripts/{script_id}/content` returns UTF-8 script content only.
  Content is capped at 1 MiB, must be a supported script type, and cannot escape
  the scripts root through `..`, absolute paths, or symlinks.
- Tree/content routes reject `.omakure`, `.history`, and `.git` metadata paths.

## CLI / HTTP Parity Matrix

| CLI | HTTP | Shared operation |
|---|---|---|
| `omakure config --json` | `GET /v1/config` | `config_summary` |
| workspace summary | `GET /v1/workspace` | `workspace_summary` |
| `omakure scripts --json` | `GET /v1/scripts` | `list_scripts` |
| `omakure search <query> --json` | `GET /v1/search?q=...` | `search_scripts` |
| `omakure describe <script> --json` | `GET /v1/scripts/{script_id}` | `describe_script` |
| script browser | `GET /v1/tree`, `GET /v1/tree/{path}` | `list_tree` |
| script content | `GET /v1/scripts/{script_id}/content` | `read_script_content` |
| `omakure doctor` | `GET /v1/doctor` | `doctor_report` |
| `omakure history list --json` | `GET /v1/runs` | `list_runs` |
| `omakure history show <run_id> --json` | `GET /v1/runs/{run_id}` | `show_run` |
| `omakure history traces <run_id> --json` | `GET /v1/runs/{run_id}/traces` | `list_traces` |
| `omakure queue stats --json` | `GET /v1/queue/stats` | `queue_stats` |
| `omakure queue add <script> --json` | `POST /v1/runs` | `enqueue_run` |
| `omakure queue cancel <run_id> --json` | `POST /v1/runs/{run_id}/cancel` | `cancel_run` |
| `omakure queue dead-letter <run_id> --json` | `POST /v1/runs/{run_id}/dead-letter` | `dead_letter_run` |
| `omakure battery list --json` | `GET /v1/batteries` | `list_batteries` |
| `omakure battery add <url> --json` | `POST /v1/batteries` | `add_battery` |
| `omakure battery sync <name> --json` | `POST /v1/batteries/{battery_id}/sync` | `sync_battery` |
| `omakure battery inspect <name> --json` | `GET /v1/batteries/{battery_id}` | `inspect_battery` |
| `omakure battery scripts <name> --json` | `GET /v1/batteries/{battery_id}/scripts` | `list_battery_scripts` |
| `omakure battery install <name> <script-id> --json` | `POST /v1/batteries/{battery_id}/scripts/{script_id}/install` | `install_battery_script` |
| `omakure battery remove <name> --json` | `DELETE /v1/batteries/{battery_id}` | `remove_battery` |

## Shared Operations

HTTP routes call shared operations for these core resources:

- `config_summary`
- `doctor_report`
- `workspace_summary`
- `search_scripts`
- `list_tree`
- `read_script_content`
- `list_scripts`
- `describe_script`
- `script_schema` via the `describe_script` operation output
- `list_runs`
- `show_run`
- `list_traces`
- `queue_stats`
- `enqueue_run`
- `cancel_run`
- `dead_letter_run`

These operations own validation and stable errors. HTTP route handlers must not
call CLI modules and must not open SQLite directly.

The following surfaces are intentionally deferred from HTTP until a separate
security/lifecycle design exists:

- `omakure update`: mutates binary/scripts from a remote release.
- `omakure uninstall`: destructive local operation.
- `omakure serve`: daemon lifecycle management.
- `omakure queue worker`: long-running process lifecycle.
- inline `omakure run`: synchronous execution surface; use `POST /v1/runs` to
  enqueue instead.
- HTTP trace ingestion: changes the trust model for script-authored telemetry.

## Write Audit Expectations

V1 write endpoints must leave an audit trail equivalent to their CLI
operation path. At minimum, writes must create or transition rows through the
existing run state machine or Battery registry/provenance records. Future
structured HTTP request logs may be added, but they must redact bearer tokens.

## Deployment Model

Loopback mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api
```

Internal container network mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --bind 0.0.0.0:7878 --allow-non-loopback
```

Publishing the API to a host port is a deployment choice outside v1's safety
guarantees. Do not document this as public-internet safe.

Container/internal-network guidance:

- Prefer loopback for local tools and sidecars on the same host.
- Use `--allow-non-loopback` only when another trusted process cannot reach
  loopback, such as a private container network.
- Put network policy, firewall rules, or reverse-proxy ACLs in front of any
  non-loopback bind.
- Do not enable permissive browser CORS in v1.
- Do not expose this API directly to the public internet; v1 has no OAuth,
  RBAC, sessions, or browser threat model.

Operational safety notes:

- Keep `OMAKURE_API_TOKEN` out of scripts and environment files used by scripts.
- Rotate the token like any other management secret.
- Request bodies are limited to 1 MiB.
- Battery cache content is still untrusted. HTTP can list, sync, inspect, and
  install through Battery operations, but cannot execute scripts directly from
  `.omakure/batteries/cache/`.
- HTTP Battery registration is restricted to `https://` sources, so non-loopback
  token holders cannot ask the server to import arbitrary local repositories.
- Queue/run writes use the existing SQLite run state machine; invalid state
  transitions return `conflict` instead of bypassing the workflow.

## Validation Gates

Every HTTP implementation task must pass:

```bash
rtk cargo test
rtk mise run lint
```

Before shipping the full API plan, run an assurance pass that includes at
least auth bypass, bind guard, token leakage, request-size, operation parity,
and Battery trust-boundary checks.
