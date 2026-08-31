# HTTP API Contract

This is the v1 contract for Omakure's internal HTTP management API.

## Goal

Expose Omakure through a small internal HTTP API in the same binary so a
trusted local process, sidecar, or internal-network container can manage a
workspace without shelling out to the CLI.

HTTP is an adapter. It must not own business logic. Each route translates an
HTTP request into a shared operation request, calls `src/operations/*`, then
serializes the operation result.

## Ownership

This document owns the HTTP route inventory, authentication and scope
semantics, JSON envelopes and errors, request limits, CLI/HTTP adapter parity,
and request audit behavior. For bind addresses, process topology, deploy
policy, workers and scheduler lifecycle, containers, volumes, readiness
operation, and certification/smoke procedures, see the canonical
[deployment guide](deployment.md).

## Node management

Node routes use the shared machine-state operations and never access
`.history/runs.sqlite`. Their bearer scopes are deliberately separate:

| Route group | Required token scope |
|---|---|
| `GET /v1/node/status`, `GET /v1/node/health`, `GET /v1/node/signals`, `GET /v1/node/peers` | `node:read` |
| `GET /v1/node/discovery` | `discovery:read` |
| `POST /v1/node/init`, `POST /v1/node/cues`, `POST /v1/node/baselines`, `POST /v1/node/baseline/rollback` | `node:write` |
| `POST /v1/node/peers`, `PATCH /v1/node/peers/:node_id/capabilities`, `POST /v1/node/peers/:node_id/revoke` | `trust:write` |
| `GET /v1/node/enrollments` | `enrollment:read` |
| `POST /v1/node/enrollments`, `POST /v1/node/enrollments/:node_id/approve`, `POST /v1/node/enrollments/:node_id/reject`, `POST /v1/node/enrollment/bundle` | `enrollment:write` |

`discovery:read` returns a bounded LAN discovery view. `trust:write` permits
manual trust import, capability updates, and revocation. All of these routes
also pass the applicable general `routes.read` or `routes.writes` deploy gate.
The `routes.node` gate covers every `/v1/node/...` route; `routes.trust` gates
write routes in the peer group; and `routes.enrollment` gates write routes in
the enrollment group. Deploy route gates are evaluated before token scopes.
Trust mutation bodies must include `confirmed: true`, a non-empty `actor`, and a
non-empty `reason`.

Routes are `GET /v1/node/status`, `GET /v1/node/discovery`, `POST
/v1/node/init`, `GET /v1/node/health`, `GET /v1/node/signals`, `POST
/v1/node/cues`, `POST /v1/node/baselines`, `POST
/v1/node/baseline/rollback`, `GET` and `POST /v1/node/peers`, `GET` and `POST
/v1/node/enrollments`, `POST /v1/node/enrollments/:node_id/approve`, `POST
/v1/node/enrollments/:node_id/reject`, `POST /v1/node/enrollment/bundle`, `PATCH
/v1/node/peers/:node_id/capabilities`, and `POST
/v1/node/peers/:node_id/revoke`. Responses use the standard JSON envelope.
Private keys, plaintext secret values, revocation reasons, and unbounded audit
history are never returned.

`GET /v1/node/discovery` returns the node service's current bounded in-memory
observation snapshot. It does not start a scan. The CLI `omakure node discovery`
command instead starts a temporary bounded listener, waits for its requested
scan interval, and returns a fresh scan snapshot; neither path creates trust or
a session.

`GET /v1/node/health` returns the Health Plane fleet-status projection: one row
per actively trusted peer with its presence (`unknown`, `online`, `stale`,
`offline`), its `baseline_status` (`unknown`, `none`, `in_sync`, `drifted`), its
latest Profile, and its latest Pulse, plus fleet totals for both. It is current status
only - there is no history, no series, and no alert surface - and it renders
exactly the same protocol-neutral operation as `omakure node health --json`. It
is read-only: no HTTP route writes Health Plane state, and the only writer is
the authenticated node-to-node exchange over the direct transport. See the
[Health Plane contract](internal/health-plane-contract.md) for the frozen
presence windows, bounds, and privacy classes.

`POST /v1/node/baseline/rollback` puts *this* node back on the one baseline it
retained before the one it is running. It takes `{"confirmed": true}` under
`node:write`, reaches no peer, and re-verifies the retained set against the
publishers this node names today: a publisher revoked since the original install
makes it fail with `forbidden`, and a node with nothing retained answers
`not_found` rather than reporting a rollback that changed nothing. There is no
node-to-node message kind that asks a Performer to change version; see the
[Baseline delivery contract](internal/baseline-delivery.md).

`GET /v1/node/signals` returns the closed lifecycle Signal feed: at most 64
entries, newest first, retained for seven days, across exactly three kinds
(`enrolled`, `revoked`, `run-completed`). `enrolled` and `revoked` are
projected from this node's authoritative append-only trust log and carry
`source: "local"`; `run-completed` is reported by a Performer over the direct
transport and carries that peer's node id as its `source`. The response also
carries the per-Performer `cursor`, `stored`, `held`, and `gap` state, because
the ordering rules stall a feed rather than admit a hole. It is read-only,
gated by the same `node:read` capability, and renders exactly the same
protocol-neutral operation as `omakure node signals --json`. There are no
subscriptions, webhooks, alert routes, or user-defined Signal kinds.

## Non-Goals

- Public internet API.
- Browser-facing CORS support.
- OAuth, OIDC, or session login (scoped bearer tokens are supported).
- Distributed queue across hosts.
- Shared SQLite over network filesystems.
- Direct execution of Battery scripts from Git cache.
- A second product surface with behavior that differs from CLI operations.

## Serving and deployment

The HTTP surface is served by `omakure api` or `omakure node serve`.
Invocation, bind guards, topology, workers, scheduler lifecycle, and readiness
are canonical in the [deployment guide](deployment.md#topology-choices);
authentication and scope matching are defined in this contract below.

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

Tokens generated before the `token_selector` optimization (bare
`omk_live_<64 hex>`, with no embedded id) still authenticate: verification
falls back to checking every enabled token hash for that shape. New tokens
from `omakure token generate` embed the id
(`omk_live_<hex id>_<64 hex>`) and require only one Argon2id verification.
Regenerate and redistribute old-format tokens when convenient for the faster
path; there is no forced cutover.


### Legacy single token

Token source: `OMAKURE_API_TOKEN` when no tokens file is configured.

- Internal token id is `legacy` with scopes `*`; route access still uses
  process-wide `--capability`.
- Reject empty, short (< 32 bytes), or known-default tokens.
- Constant-time compare of the presented legacy token.

### Scope matching and legacy capabilities

Multi-token bearer scopes use these matching rules:

- `*` satisfies every required scope.
- `env:read`/`envs:read`, `env:write`/`envs:write`,
  `env:activate`/`envs:activate`, and `env:use`/`envs:use` are aliases.
- Coarse write grants cover finer actions, but never the reverse:
  `runs:write` covers `runs:enqueue`, `runs:cancel`, and
  `runs:dead-letter`; `batteries:write` covers `batteries:add`,
  `batteries:sync`, `batteries:install`, and `batteries:remove`.
- `config:read` covers `doctor:read` and `workspace:read`; `scripts:read`
  covers `search:read`.

Legacy `--capability` values are normalized into broad route-capability
classes before checks. Thus `env:read`/`envs:read`, `env:write`/`envs:write`,
`env:activate`/`envs:activate`, and `env:use`/`envs:use` are aliases;
`doctor:read` and `workspace:read` (the `config:read` class), and
`search:read` (the `scripts:read` class), and run/Battery action spellings
map to the same classes as their canonical capabilities. Unlike file scopes,
these legacy action spellings
`runs:enqueue`, `runs:cancel`, `runs:dead-letter`, `batteries:add`,
`batteries:sync`, `batteries:install`, and `batteries:remove` therefore grant
their entire `runs:write` or `batteries:write` class. `--capability` is ignored
when `--tokens-file` is set.

Legacy `--capability` is repeatable and accepts:
`config:read`, `scripts:read`, `env:read`, `envs:read`, `env:write`,
`envs:write`, `env:activate`, `envs:activate`, `env:use`, `envs:use`,
`secrets:use`, `secrets:read-metadata`, `credentials:use`, `runs:read`,
`runs:write`, `runs:enqueue`, `runs:cancel`, `runs:dead-letter`,
`batteries:read`, `batteries:write`, `batteries:add`, `batteries:sync`,
`batteries:install`, `batteries:remove`, `admin:status`, `node:read`,
`node:write`, `trust:write`, `enrollment:read`, `enrollment:write`,
`discovery:read`, `doctor:read`, `workspace:read`, `search:read`, and
`all`.

Legacy secret access is a separate allow-list. Repeat `--secret-ref` with
`secret://provider/key` or `secret://provider/*`; an empty list denies
provider refs. `--capability all` grants route capabilities but does not
bypass this list, so unrestricted file/provider refs require
`--secret-ref '*'`. That wildcard does not grant process-environment refs:
enumerate each exact `secret://env/NAME` (the `secret://env:*` spelling
normalizes to that provider form, while `--secret-ref 'secret://env/*'` is
ignored).
Secret-backed run fields/arguments require `secrets:use` and a matching ref;
private HTTPS Battery `token_ref` additionally requires `credentials:use` and
its matching ref.

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

The same illegal queue transition is rendered as CLI `invalid_argument` but as
an HTTP `conflict` error with status `409`. This is an adapter-level error
mapping; the underlying state-machine transition is rejected in both cases.

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
Optional readiness gates are configured by `omakure node serve`; their
deployment semantics are defined in the [deployment guide](deployment.md).

Authenticated operator status (scope `admin:status`, or legacy `*`):

```http
GET /v1/admin/status
```

Returns readiness details (worker/scheduler gates and liveness) plus auth-file
load/reload state (`mode`, `token_count`, `last_reload_ok`,
`last_reload_error`, `last_reload_at_ms`). It never returns token IDs, hashes,
plaintext secrets, or the tokens-file path.

### Route groups and required scopes

Every authenticated route also passes the deploy-policy route gates. The
required bearer scopes for ordinary (non-node) routes are:

| Route group | Methods and paths | Required scope |
|---|---|---|
| Configuration and diagnostics | `GET /v1/config`, `/v1/doctor`, `/v1/workspace` | `config:read` |
| Scripts and tree | `GET /v1/search`, `/v1/tree...`, `/v1/scripts...` | `scripts:read` |
| Run and queue reads | `GET /v1/runs...`, `GET /v1/queue/stats` | `runs:read` |
| Run enqueue | `POST /v1/runs` | `runs:enqueue` |
| Run cancellation/dead-letter | `POST /v1/runs/:run_id/cancel`, `/dead-letter` | `runs:cancel` or `runs:dead-letter` |
| Environment reads | `GET /v1/envs...` | `envs:read` |
| Environment writes | `POST`, `PUT`, `PATCH`, or `DELETE /v1/envs...` | `envs:write` |
| Environment activation | `POST /v1/envs/:name/activate`, `DELETE /v1/envs/active` | `envs:activate` |
| Battery reads | `GET /v1/batteries...` | `batteries:read` |
| Battery registration/sync | `POST /v1/batteries`, `/v1/batteries/:battery_id/sync` | `batteries:add` or `batteries:sync` |
| Battery installation/removal | `POST .../install`, `DELETE /v1/batteries/:battery_id` | `batteries:install` or `batteries:remove` |
| Secret metadata | `GET /v1/secrets` | `secrets:read-metadata` |
| Admin status | `GET /v1/admin/status` | `admin:status` |

Run enqueue may additionally require `envs:use` for explicit environment
selection and `secrets:use` for secret references; schema defaults can also
require `envs:use` or `secrets:use`. Node route scopes are listed separately
above.

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
GET /v1/node/status
GET /v1/node/discovery
GET /v1/node/health
GET /v1/node/signals
GET /v1/node/peers
GET /v1/node/enrollments
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
POST /v1/node/init
POST /v1/node/cues
POST /v1/node/baselines
POST /v1/node/baseline/rollback
POST /v1/node/peers
POST /v1/node/enrollments
POST /v1/node/enrollments/{node_id}/approve
POST /v1/node/enrollments/{node_id}/reject
POST /v1/node/enrollment/bundle
PATCH /v1/node/peers/{node_id}/capabilities
POST /v1/node/peers/{node_id}/revoke
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

## CLI / HTTP parity

Recorded in `cli-http-parity.md`, which is the single copy: it carries the
status and the notes as well as the mapping, and a second hand-maintained
table here had already drifted from it.

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

## Deployment and operations

Deployment procedures and topology are canonical in the
[deployment guide](deployment.md). It covers loopback and trusted internal
network binds, policy load order and hard gates, workers and scheduler
lifecycle, startup/readiness, containers, volumes, SQLite placement,
certification, and smoke operation. This API contract retains the route,
authentication, scope, envelope, limit, adapter, and audit semantics that
apply in every deployment.
