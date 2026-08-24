# HTTP API Contract

This is the v1 contract for Omakure's internal HTTP management API.

## Goal

Expose Omakure through a small internal HTTP API in the same binary so a
trusted local process, sidecar, or internal-network container can manage a
workspace without shelling out to the CLI.

HTTP is an adapter. It must not own business logic. Each route translates an
HTTP request into a shared operation request, calls `src/operations/*`, then
serializes the operation result.

## Node management

Node routes use the shared machine-state operations and never access
`.history/runs.sqlite`. `node:read` permits public status and bounded peer
listing; `node:write` permits explicit initialization; `trust:write` permits
manual import, capability updates, and revocation. Trust mutation bodies must
include `confirmed: true`, a non-empty `actor`, and a non-empty `reason`.

Routes are `GET /v1/node/status`, `POST /v1/node/init`, `GET` and `POST
/v1/node/peers`, `PATCH /v1/node/peers/:node_id/capabilities`, and `POST
/v1/node/peers/:node_id/revoke`. Responses use the standard JSON envelope.
Private keys, plaintext secret values, revocation reasons, and unbounded audit
history are never returned.

## Non-Goals

- Public internet API.
- Browser-facing CORS support.
- OAuth, OIDC, or session login (scoped bearer tokens are supported).
- Distributed queue across hosts.
- Shared SQLite over network filesystems.
- Direct execution of Battery scripts from Git cache.
- A second product surface with behavior that differs from CLI operations.

## Framework Decision

Use `axum` with `tokio` for v1.

Why:

- Middleware and extractors keep auth, body limits, and route state testable.
- `tower` service testing lets endpoint tests run without binding real ports.
- The async runtime is contained behind `omakure api`; local CLI commands remain
  synchronous.

## Command Contract

```bash
# Preferred: multi-token file (per-token scopes; --capability ignored)
omakure api --bind 127.0.0.1:7878 --tokens-file /run/secrets/omakure_tokens.toml

# Legacy single-token mode
omakure api --bind 127.0.0.1:7878 \
  --capability config:read \
  --capability scripts:read \
  --capability runs:read \
  --capability runs:write \
  --capability env:read
omakure api --bind 0.0.0.0:7878 --allow-non-loopback \
  --capability config:read \
  --capability scripts:read \
  --capability runs:read \
  --capability env:read \
  --capability env:write
```

### Engine (single-process deploy)

`omakure engine` runs the same HTTP surface as `omakure api`, plus optional
in-process queue workers and the existing schedule scanner, with coordinated
SIGTERM shutdown. Auth is identical (`--tokens-file` / `OMAKURE_TOKENS_FILE`,
or legacy `OMAKURE_API_TOKEN` + `--capability` / `--secret-ref`).

```bash
# API-only (≈ omakure api)
omakure engine --workers 0 --no-scheduler --tokens-file /run/secrets/tokens.toml

# HTTP + one worker + scheduler (defaults: --workers 1, scheduler on)
omakure engine --workers 1 --scheduler --tokens-file /run/secrets/tokens.toml

# Fail ready until configured loops are alive
omakure engine --workers 2 \
  --readiness-requires-worker \
  --readiness-requires-scheduler
```

| Flag | Default | Meaning |
|---|---:|---|
| `--workers <n>` | `1` | Embedded queue workers; `0` = API only |
| `--scheduler` / `--no-scheduler` | on | Enable/disable `scheduler_tick` in-process |
| `--readiness-requires-worker` | off | `/v1/ready` fails if workers configured but not alive |
| `--readiness-requires-scheduler` | off | `/v1/ready` fails if scheduler enabled but not alive |
| `--tokens-file` | none | Multi-token Argon2id TOML (`OMAKURE_TOKENS_FILE`) |

Defaults and guards:

- Default bind: `127.0.0.1:7878`.
- Loopback bind is allowed without extra flags.
- Non-loopback bind requires `--allow-non-loopback`.
- `0.0.0.0` and `::` count as non-loopback.
- Binding must fail before listening when the guard is not satisfied.
- **Tokens-file mode:** scopes come from each token entry; process-wide
  `--capability` is ignored. Prefer plan scopes (`runs:enqueue`,
  `envs:read`, …). Legacy capability names (`env:read`, `runs:write`) are
  accepted as aliases.
- **Legacy mode** (no tokens file): capabilities are denied by default.
  Grant them with repeated `--capability` flags: `config:read`,
  `scripts:read`, `env:read`/`envs:read`, `env:write`/`envs:write`,
  `env:activate`/`envs:activate`, `env:use`/`envs:use`, `secrets:use`,
  `secrets:read-metadata`, `credentials:use`, `runs:read`,
  `runs:write`/`runs:enqueue`, `batteries:read`, `batteries:write`, or `all`.
- Read endpoints are gated: config/workspace/doctor require `config:read`
  (or `doctor:read` / `workspace:read` in tokens-file mode),
  script/search/tree require `scripts:read` (or `search:read`), runs require
  `runs:read`, Battery read requires `batteries:read`, secrets metadata
  requires `secrets:read-metadata`.
- Secret provider refs are denied by default in legacy mode. Grant exact refs
  or provider wildcards with repeated `--secret-ref`. Private HTTPS Battery
  `token_ref` also needs `credentials:use`.

## Authentication Contract

Every endpoint except `/v1/health` and `/v1/ready` requires:

```http
Authorization: Bearer <token>
```

### Multi-token file (preferred)

```bash
omakure token generate --id ci-deployer \
  --scope runs:enqueue --scope runs:read --scope scripts:read --json
# copy data.token once; store data.tokens_file_entry in the secrets file
export OMAKURE_TOKENS_FILE=/run/secrets/omakure_tokens.toml
# or: omakure api --tokens-file /run/secrets/omakure_tokens.toml
```

TOML shape:

```toml
version = 1

[[tokens]]
id = "ci-deployer"
hash = "$argon2id$v=19$m=65536,t=3,p=1$..."
scopes = ["runs:enqueue", "runs:read", "scripts:read"]
enabled = true
```

Rules:

- Plaintext never appears in the file; hashes are Argon2id PHC strings.
- Token ids are unique; disabled tokens are ignored.
- Missing/unknown bearer → `401`; authenticated token missing scope → `403`.
- Internal `AuthContext { token_id, scopes }` is available to handlers.
- On Unix, `SIGHUP` reloads the tokens file; a failed reload keeps the last
  valid set. Reload health is visible on `GET /v1/admin/status` (never drops
  the last valid auth set on parse/I/O failure).
- Generate with `omakure token generate` (prefix `omk_live_`). Optional
  `--append PATH --confirmed` appends the TOML entry.

### Legacy single token

Token source: `OMAKURE_API_TOKEN` when no tokens file is configured.

- Internal token id is `legacy` with scopes `*`; route access still uses
  process-wide `--capability`.
- Reject empty, short (< 32 bytes), or known-default tokens.
- Constant-time compare of the presented legacy token.

Shared rules:

- Redact token values from logs, errors, JSON responses, and test fixtures.
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

Health and readiness are unauthenticated:

```http
GET /v1/health
GET /v1/ready
```

`GET /v1/ready` returns only a minimal `{ "status": "ready" | "not_ready" }`
payload (HTTP 200 or 503). It must not expose token IDs, paths, or secrets.
Optional `--readiness-requires-worker` / `--readiness-requires-scheduler` on
`omakure engine` gate readiness on those loops.

Authenticated operator status (scope `admin:status`, or legacy `*`):

```http
GET /v1/admin/status
```

Returns readiness details (worker/scheduler gates and liveness) plus auth-file
load/reload state (`mode`, `token_count`, `last_reload_ok`,
`last_reload_error`, `last_reload_at_ms`). It never returns token IDs, hashes,
plaintext secrets, or the tokens-file path.

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
GET /v1/envs
GET /v1/envs/{name}
GET /v1/batteries
GET /v1/batteries/{battery_id}
GET /v1/batteries/{battery_id}/scripts
GET /v1/secrets
```

`GET /v1/secrets` returns **metadata only** (`id`, `source`, `delivery`,
`allowed_targets`) for refs allowed by the token ACL. It never returns secret
values. Requires scope `secrets:read-metadata` and deploy policy
`secrets.metadata_endpoint = true` (otherwise `404`).

Write endpoints require auth:

```http
POST /v1/runs
POST /v1/runs/{run_id}/cancel
POST /v1/runs/{run_id}/dead-letter
POST /v1/envs
PUT /v1/envs/{name}
PATCH /v1/envs/{name}
DELETE /v1/envs/{name}
POST /v1/envs/{name}/activate
DELETE /v1/envs/active
PUT /v1/envs/{name}/params/{key}
DELETE /v1/envs/{name}/params/{key}
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
  "env": "prod",
  "secret_fields": { "TOKEN": "secret://prod/token" },
  "run_id": "optional-caller-id",
  "actor": "agent",
  "reason": "why this was queued",
  "priority": 10,
  "timeout_ms": 60000,
  "parent_run_id": null,
  "cron_schedule_id": null
}
```

Defaults: `args=[]`, `secret_fields={}`, `actor="human"`, `priority=0`.
`env` names a managed environment file under `.omakure/envs/` and overlays it
for the queued run when the worker drains it. `secret_fields` supplies
reconstructable `secret://...` references for schema fields whose `Type` is
`secret`; queued HTTP runs reject plaintext `secret_fields` because the worker
cannot reconstruct them without persisting plaintext. Forwarded `args` for
secret schema fields must also use `secret://...` refs. Responses and stored run
args redact plaintext secret values as `<redacted>` and retain provider refs.

```json
POST /v1/runs/{run_id}/cancel
{ "reason": "optional" }

POST /v1/runs/{run_id}/dead-letter
{ "reason": "optional" }

POST /v1/envs
{
  "name": "prod",
  "params": [
    { "key": "HOST", "value": "prod.example.com" },
    { "key": "API_KEY", "value": "secret://prod/api_key" }
  ]
}

PUT /v1/envs/prod
{
  "params": [
    { "key": "HOST", "value": "prod.example.com" }
  ]
}

PATCH /v1/envs/prod
{
  "params": [
    { "key": "REGION", "value": "eastus" }
  ]
}

PUT /v1/envs/prod/params/API_KEY
{ "value": "secret://prod/api_key" }

POST /v1/batteries
{
  "name": "azure",
  "git_url": "https://example.invalid/azure.git",
  "requested_ref": "main",
  "token_ref": "secret://creds/git_token"
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

Optional `token_ref` enables private HTTPS clone/fetch via `GIT_ASKPASS`. The
registry stores `auth.method` + `auth.token_ref` only — never plaintext.
Private HTTPS add/sync require `credentials:use` plus
`sources.allow_private_https_batteries` in deploy policy. Embedded URL
credentials are rejected.

Read query parameters and safety policy:

- `GET /v1/config` returns the full config shape, but HTTP masks every active
  environment value. Plaintext env diagnostics are CLI-only.
- `GET /v1/envs` lists managed `.omakure/envs/*.conf` files and marks the
  active one. `GET /v1/envs/{name}` returns parsed entries with sensitive values
  masked as `****`.
- Env writes validate managed environment names and keys, reject path escapes,
  write single-line values atomically, and never operate outside
  `.omakure/envs/`.
- Bearer auth is required for every env endpoint. Internal capability policy
  checks apply `EnvRead`, `EnvWrite`, `EnvActivate`, `EnvUse`, and
  `SecretProviderUse`; a token without the required capability receives
  `403 forbidden`.
- `POST /v1/runs` requires `EnvUse` when the body includes `env`. It requires
  `SecretProviderUse` when the body includes `secret_fields` or any forwarded
  arg value beginning with `secret://`; provider refs are also checked against
  the token's allowed ref ACL before enqueue and the allowed ref set is sealed
  with the queued run for worker-time resolution.
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
| `omakure env list --json` | `GET /v1/envs` | `list_envs` |
| `omakure env create <name> ... --json` | `POST /v1/envs` | `create_env` |
| `omakure env show <name> --json` | `GET /v1/envs/{name}` | `show_env` |
| `omakure env replace <name> ... --json` | `PUT /v1/envs/{name}` | `replace_env` |
| `omakure env set <name> KEY=VALUE --json` | `PATCH /v1/envs/{name}`, `PUT /v1/envs/{name}/params/{key}` | `set_param` |
| `omakure env remove <name> <key> --json` | `DELETE /v1/envs/{name}/params/{key}` | `remove_param` |
| `omakure env activate <name> --json` | `POST /v1/envs/{name}/activate` | `activate_env` |
| `omakure env deactivate --json` | `DELETE /v1/envs/active` | `deactivate_env` |
| `omakure env delete <name> --json` | `DELETE /v1/envs/{name}` | `delete_env` |
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
- `list_envs`
- `create_env`
- `show_env`
- `replace_env`
- `set_param`
- `remove_param`
- `activate_env`
- `deactivate_env`
- `delete_env`
- `list_batteries`
- `add_battery`
- `sync_battery`
- `inspect_battery`
- `list_battery_scripts`
- `install_battery_script`
- `remove_battery`

These operations own validation and stable errors. HTTP route handlers do not
call CLI modules and do not open SQLite directly.

The current HTTP API does not expose these CLI surfaces:

- `omakure update`: mutates binary/scripts from a remote release.
- `omakure uninstall`: destructive local operation.
- `omakure serve`: daemon lifecycle management.
- `omakure queue worker`: long-running process lifecycle.
- inline `omakure run`: synchronous execution surface; use `POST /v1/runs` to
  enqueue instead.
- HTTP trace ingestion.

## Write Audit Expectations

V1 write endpoints leave an audit trail equivalent to their CLI
operation path. At minimum, writes must create or transition rows through the
existing run state machine or Battery registry/provenance records. HTTP errors
and responses redact bearer tokens.

Authenticated HTTP requests also emit structured request audit lines on stderr
(`omakure.http_audit {…}`) with `token_id`, `method`, `path`, `outcome`, and
`status`. The `Authorization` header and raw bearer secrets are never logged.
Operators correlate enqueue/cancel/dead-letter with `token_id` via these
request logs (no runs.sqlite schema change in this increment).

## Deployment Model

Loopback mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api
# or single-process:
omakure engine --workers 1
```

Internal container network mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --bind 0.0.0.0:7878 --allow-non-loopback
# or:
omakure engine --bind 0.0.0.0:7878 --allow-non-loopback --workers 2
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

## Validation

Use the normal repository checks after changing the HTTP API:

```bash
cargo test
mise run lint
```
