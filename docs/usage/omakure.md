# `omakure`
- **Version:** 0.3.0

Omakure - CLI for running and scheduling automation scripts.

Run `omakure` with no arguments to print this help.

CLI surfaces:
 run <SCRIPT>          execute a script directly
 queue add <SCRIPT>    push a job; `queue worker` drains it
 serve                 run the cron scheduler daemon
 history list|show     query past runs (SQLite-backed)
 scripts|describe|search   inspect the script catalogue

AI integration: pass `--json` on supported subcommands to emit a `{ ok, data, error, schema_version }` envelope; run `omakure help-ai` for the full machine-readable capability surface.


- **Usage:** `omakure [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

## Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure run`

- **Usage:** `omakure run [FLAGS] <SCRIPT> [ARGS]…`

Run a script directly

### Arguments
- **`<SCRIPT>`** — Script name or path
- **`[ARGS]…`** — Arguments forwarded to the script

### Flags
- **`--actor <ACTOR>`** — Actor tag recorded in the run history (default: `human`)

  **Default:** `human`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--reason <REASON>`** — Optional free-form reason recorded in the run history
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--run-id <RUN_ID>`** — Caller-provided run id; otherwise a fresh id is generated
- **`--parent-run-id <PARENT_RUN_ID>`** — Optional parent run id, for chained agent workflows
- **`--no-prompt`** — Fail with a structured error if any required field is missing instead of attempting to read stdin / open a TTY. Implied by `--json`

  **Default:** `false`
- **`--env-file <PATH>`** — Path to an env file whose `KEY=value` pairs are injected into the script process for this run only. Values override the managed active env for the same key, but omakure-reserved vars (`OMAKURE_RUN_ID`, `OMAKURE_SCRIPTS_DIR`) always win. A missing or unreadable path is a hard error.

  Example: `omakure run deploy --env-file ./.venv.env -- --target prod`
- **`--secret <FIELD=VALUE>…`** — Direct secret field input as `FIELD=value`. The value is supplied to secret schema fields for this run and is redacted from stored args

## `omakure doctor`

- **Usage:** `omakure doctor [--scripts-dir <SCRIPTS_DIR>] [--json]`
- **Aliases:** `check`

Check runtime dependencies and workspace

Verifies required interpreters (`git`, `bash`, `jq`), optional ones (`powershell`, `python`), workspace layout (`.omakure/`, history dir, workspace config), and that every script's embedded schema parses. Exits 1 if any required check fails. `--json` is currently ignored by this subcommand.

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure scripts`

- **Usage:** `omakure scripts [FLAGS]`

List available scripts

### Flags
- **`--tag <TAG>…`** — Filter by tag (repeatable; AND semantics, case-sensitive literal match against the script's embedded `Tags` field)
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure describe`

- **Usage:** `omakure describe [--scripts-dir <SCRIPTS_DIR>] [--json] <SCRIPT>`

Show the full schema of one script

### Arguments
- **`<SCRIPT>`** — Script name or path

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure search`

- **Usage:** `omakure search [FLAGS] [QUERY]`

Search the script index

### Arguments
- **`[QUERY]`** — Free-text query (matches name, description, tags, fields)

  **Default:** ``

### Flags
- **`--tag <TAG>…`** — Filter by tag (repeatable; AND semantics, case-sensitive literal match against the script's embedded `Tags` field)
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure history`

- **Usage:** `omakure history [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Query the run history

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure history list`

- **Usage:** `omakure history list [FLAGS]`

List recent runs

### Flags
- **`--script <SCRIPT>`** — Filter by script name or path substring
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--actor <ACTOR>`** — Filter by actor tag (e.g. `human`, `ai`)
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--since <SINCE>`** — Only runs since this duration ago (e.g. `1d`, `30m`, `12h`)
- **`--until <UNTIL>`** — Only runs until this duration ago
- **`--success`** — Only successful runs

  **Default:** `false`
- **`--failure`** — Only failed runs

  **Default:** `false`
- **`--limit <LIMIT>`** — Maximum number of rows to return
- **`--state <STATE>…`** — Filter by run state (repeatable; logical OR within the flag). Valid values: queued, running, completed, failed, cancelled, timed_out, dead_letter. Mutually exclusive with `--state-set`
- **`--state-set <STATE_SET>`** — Filter by a named state group: `in_flight` (queued+running), `terminal` (everything else), or `all`. Default when neither `--state` nor `--state-set` is set: `terminal` so existing callers see no behavior change

## `omakure history show`

- **Usage:** `omakure history show [--scripts-dir <SCRIPTS_DIR>] [--json] <RUN_ID>`

Show one run by id

### Arguments
- **`<RUN_ID>`** — Run id (as printed by `omakure run --json` or `omakure history list`)

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure history tail`

- **Usage:** `omakure history tail [FLAGS]`

Print the most recent N runs (no --follow in v1)

### Flags
- **`--limit <LIMIT>`** — Number of rows to print (default: 10)

  **Default:** `10`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--follow`** — Reserved for future use; rejected with error.code = "not_implemented"

  **Default:** `false`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure history stats`

- **Usage:** `omakure history stats [--scripts-dir <SCRIPTS_DIR>] [--json]`

Aggregate counts per state and per actor

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure history traces`

- **Usage:** `omakure history traces [FLAGS] <RUN_ID>`

Read the structured trace stream of one run

### Arguments
- **`<RUN_ID>`** — Run id

### Flags
- **`--level <LEVEL>`** — Minimum level (debug, info, warn, error). Defaults to `debug` (returns every record)
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--since-sequence <SINCE_SEQUENCE>`** — Return only entries with `sequence > N`. Used by agents for incremental fetches
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure queue`

- **Usage:** `omakure queue [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Push, cancel, drain, and inspect the run queue

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure queue add`

- **Usage:** `omakure queue add [FLAGS] <SCRIPT> [ARGS]…`

Push a job onto the queue

### Arguments
- **`<SCRIPT>`** — Script name or path
- **`[ARGS]…`** — Arguments forwarded to the script (after `--`)

### Flags
- **`--actor <ACTOR>`** — Actor tag recorded on the row (default: `human`)

  **Default:** `human`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--reason <REASON>`** — Optional free-form reason
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--priority <PRIORITY>`** — Higher value picked first (default 0)

  **Default:** `0`
- **`--timeout <TIMEOUT>`** — Per-job execution timeout (e.g. `30s`, `5m`, `1h`). Without this flag the job has no execution limit
- **`--parent-run-id <PARENT_RUN_ID>`** — Optional parent run id, for chained agent workflows
- **`--run-id <RUN_ID>`** — Caller-provided run id; otherwise a fresh id is generated
- **`--cron-schedule-id <CRON_SCHEDULE_ID>`** — Provenance id tying this row to a named cron schedule. Populated automatically by `omakure serve`; set manually only to replay or simulate a scheduled run

## `omakure queue cancel`

- **Usage:** `omakure queue cancel [FLAGS] <RUN_ID>`

Cancel a queued or running job

### Arguments
- **`<RUN_ID>`** — Run id to cancel

### Flags
- **`--reason <REASON>`** — Optional reason recorded on the cancelled row
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure queue dead-letter`

- **Usage:** `omakure queue dead-letter [FLAGS] <RUN_ID>`

Promote a `failed` or `timed_out` row into `dead_letter`

### Arguments
- **`<RUN_ID>`** — Run id to promote

### Flags
- **`--reason <REASON>`** — Optional reason appended to the row
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure queue worker`

- **Usage:** `omakure queue worker [FLAGS]`

Drain the queue (long-running daemon)

### Flags
- **`--concurrency <CONCURRENCY>`** — Number of parallel workers (default 1)

  **Default:** `1`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--actor-filter <ACTOR_FILTER>`** — Only claim jobs whose actor matches this tag
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--script-filter <SCRIPT_FILTER>`** — Only claim jobs whose script path or name contains this pattern

## `omakure queue stats`

- **Usage:** `omakure queue stats [--scripts-dir <SCRIPTS_DIR>] [--json]`

Aggregate counts per state and per actor

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure battery`

- **Usage:** `omakure battery [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Manage reusable Battery automation repositories

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure battery list`

- **Usage:** `omakure battery list [--scripts-dir <SCRIPTS_DIR>] [--json]`

List registered Batteries

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure battery add`

- **Usage:** `omakure battery add <FLAGS> <GIT_URL>`

Register a Battery repository source

### Arguments
- **`<GIT_URL>`** — Git repository URL or local path

### Flags
- **`--name <NAME>`** — Stable Battery name (lowercase kebab-case)
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--ref <REQUESTED_REF>`** — Branch, tag, or ref to sync

  **Default:** `main`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--token-ref <TOKEN_REF>`** — Secret ref for private HTTPS auth (`secret://provider/key`). Registry stores the ref only; sync resolves via GIT_ASKPASS

## `omakure battery sync`

- **Usage:** `omakure battery sync [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME>`

Fetch and validate a Battery checkout

### Arguments
- **`<NAME>`** — Battery name

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure battery inspect`

- **Usage:** `omakure battery inspect [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME>`

Inspect one synced Battery manifest

### Arguments
- **`<NAME>`** — Battery name

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure battery scripts`

- **Usage:** `omakure battery scripts [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME>`

List installable scripts from one Battery

### Arguments
- **`<NAME>`** — Battery name

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure battery install`

- **Usage:** `omakure battery install [FLAGS] <NAME> <SCRIPT_ID>`

Install one Battery script into the trusted scripts workspace

### Arguments
- **`<NAME>`** — Battery name
- **`<SCRIPT_ID>`** — Script id from `omakure battery scripts <name>`

### Flags
- **`--force`** — Overwrite an existing script target

  **Default:** `false`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure battery remove`

- **Usage:** `omakure battery remove [FLAGS] <NAME>`

Unregister one Battery

### Arguments
- **`<NAME>`** — Battery name

### Flags
- **`--remove-cache`** — Also delete the cached clone

  **Default:** `false`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure token`

- **Usage:** `omakure token [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Generate hashed API tokens for `--tokens-file` auth

Prints a plaintext token once (prefix `omk_live_`), its Argon2id PHC hash, and a TOML `[[tokens]]` entry. Does not append to a secrets file unless `--append` is passed with `--confirmed`.

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure token generate`

- **Usage:** `omakure token generate <FLAGS>`

Generate a plaintext token, Argon2id hash, and TOML entry

### Flags
- **`--id <ID>`** — Stable token id (logged/audited; never the secret)
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--scope <SCOPES>…`** — Scope to grant (repeatable), e.g. runs:read, scripts:read, *
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--append <APPEND>`** — Append the TOML entry to this tokens file (requires `--confirmed`)
- **`--confirmed`** — Confirm a destructive/automated `--append`

  **Default:** `false`

## `omakure api`

- **Usage:** `omakure api [FLAGS]`

Run the internal HTTP management API

Starts a loopback-only HTTP API by default at `127.0.0.1:7878`. All endpoints except `/v1/health` and `/v1/ready` require `Authorization: Bearer <token>`. Prefer `--tokens-file` / `OMAKURE_TOKENS_FILE` (per-token Argon2id scopes). Legacy `OMAKURE_API_TOKEN` still works when no tokens file is configured. Binding to non-loopback addresses requires `--allow-non-loopback`.

### Flags
- **`--bind <BIND>`** — Address to bind the HTTP API server to

  **Default:** `127.0.0.1:7878`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--allow-non-loopback`** — Explicitly allow the HTTP API to bind to non-loopback addresses

  **Default:** `false`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--policy <POLICY>`** — Deploy-only policy.toml (route groups + auth/node-service defaults). Overrides `OMAKURE_POLICY_FILE`. Separate from workspace omakure.toml
- **`--tokens-file <TOKENS_FILE>`** — Multi-token TOML file (Argon2id hashes + per-token scopes). Overrides `OMAKURE_TOKENS_FILE`. When set, process-wide `--capability` is ignored; scopes come from each token
- **`--capability <CAPABILITIES>…`** — API capability to grant in legacy single-token mode (`OMAKURE_API_TOKEN`). Repeatable. Ignored when `--tokens-file` is set. Supported: config:read, scripts:read, env:read / envs:read, env:write / envs:write, env:activate / envs:activate, env:use / envs:use, secrets:use, secrets:read-metadata, credentials:use, runs:read, runs:write / runs:enqueue, batteries:read, batteries:write, admin:status, all. Node management uses narrow node:read, node:write, and trust:write capabilities. `all` grants every route capability but does not bypass `--secret-ref` (pass `--secret-ref '*'` for unrestricted refs)
- **`--secret-ref <SECRET_REFS>…`** — Allowed secret provider ref for secrets:use / credentials:use, e.g. secret://prod/token or secret://prod/*; repeatable. Empty denies provider refs

## `omakure trace`

- **Usage:** `omakure trace [FLAGS] <MESSAGE>`

Run the machine-owned node service (HTTP API + optional workers + scheduler)

Starts the HTTP management API and optionally embeds queue workers and the existing schedule scanner in one process. Use `--workers 0 --no-scheduler` for API-only (same auth surface as `omakure api`). `GET /v1/ready` is unauthenticated and returns minimal readiness. `GET /v1/admin/status` (scope `admin:status`) exposes readiness details and token reload health without secrets. Authenticated requests emit `omakure.http_audit` lines with `token_id` (Authorization redacted). SIGTERM/SIGINT stops HTTP first, then scheduling/claiming, then drains workers. Append a structured trace event from inside a running script

### Arguments
- **`<MESSAGE>`** — Trace message

### Flags
- **`--level <LEVEL>`** — Level (debug, info, warn, error). Defaults to `info`

  **Default:** `info`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--data <DATA>`** — Optional structured payload (must parse as JSON)
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure help-ai`

- **Usage:** `omakure help-ai [--scripts-dir <SCRIPTS_DIR>] [--json]`

Print the AI capability surface as JSON

Always emits JSON (regardless of `--json`). The envelope uses the standard `{ ok, data, error, schema_version }` shape.

 `data` contains:
 trust_model   — how omakure treats AI callers
 error_codes   — the registered stable error code strings
 envelope      — a self-describing shape hint
 verbs         — AI-relevant subcommands with flags and nested subcommands (pulled from clap metadata, so it cannot drift from `--help`)
 data_shapes   — concrete examples for `run`, `history_list`, `history_show`, and `config`

Agents can cache the payload per binary version (`--version`).

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure init`

- **Usage:** `omakure init [FLAGS] [SCRIPT]`

Create a new script template

### Arguments
- **`[SCRIPT]`** — Script path

### Flags
- **`--name <SCRIPT>`** — Script path (legacy)
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--schema-json <SCHEMA_JSON>`** — Inline schema JSON or `@path/to/schema.json`. When set, the new script is generated with this schema embedded between the `OMAKURE_SCHEMA_START` / `OMAKURE_SCHEMA_END` markers instead of the default placeholder template
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--body-stdin`** — Read the script body from stdin and write it verbatim under the schema header. Useful when an agent ships both schema and body in one call

  **Default:** `false`
- **`--force`** — Overwrite an existing script of the same name

  **Default:** `false`

## `omakure env`

- **Usage:** `omakure env [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Manage named environment files

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env list`

- **Usage:** `omakure env list [--scripts-dir <SCRIPTS_DIR>] [--json]`

List named environments

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env create`

- **Usage:** `omakure env create [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME> [KEY=VALUE]…`

Create a named environment from optional `KEY=value` pairs

### Arguments
- **`<NAME>`**
- **`[KEY=VALUE]…`**

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env show`

- **Usage:** `omakure env show [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME>`

Show a named environment with sensitive values redacted

### Arguments
- **`<NAME>`**

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env set`

- **Usage:** `omakure env set [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME> <KEY=VALUE>`

Set one `KEY=value` in a named environment

### Arguments
- **`<NAME>`**
- **`<KEY=VALUE>`**

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env remove`

- **Usage:** `omakure env remove [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME> <KEY>`

Remove one key from a named environment

### Arguments
- **`<NAME>`**
- **`<KEY>`**

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env replace`

- **Usage:** `omakure env replace [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME> [KEY=VALUE]…`

Replace a named environment with the provided `KEY=value` pairs

### Arguments
- **`<NAME>`**
- **`[KEY=VALUE]…`**

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env activate`

- **Usage:** `omakure env activate [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME>`

Activate a named environment

### Arguments
- **`<NAME>`**

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env deactivate`

- **Usage:** `omakure env deactivate [--scripts-dir <SCRIPTS_DIR>] [--json]`

Deactivate the current environment

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure env delete`

- **Usage:** `omakure env delete [--scripts-dir <SCRIPTS_DIR>] [--json] <NAME>`

Delete a named environment

### Arguments
- **`<NAME>`**

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node`

- **Usage:** `omakure node [FLAGS] <SUBCOMMAND>`

Inspect and explicitly manage the machine-owned node identity and trust registry

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

### Flags
- **`--node-state-dir <STATE_DIR>`** — Deterministic test-only node state directory override
- **`--node-config <CONFIG_PATH>`** — Deterministic test-only node configuration path override

## `omakure node serve`

- **Usage:** `omakure node serve [FLAGS]`

Run the machine-owned HTTP node service with optional workers and scheduler

### Flags
- **`--bind <BIND>`** — Address to bind the HTTP API server to; defaults to node.toml `api.bind`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--direct-bind <DIRECT_BIND>`** — Optional direct transport listener address
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--allow-non-loopback`** — Explicitly allow binding to non-loopback addresses

  **Default:** `false`
- **`--allow-non-loopback-direct`** — Explicitly allow the direct transport to bind to non-loopback addresses

  **Default:** `false`
- **`--policy <POLICY>`** — Deploy-only policy.toml. Same as `omakure api --policy`
- **`--workers <WORKERS>`** — Number of embedded queue workers. `0` means API-only (no claiming)
- **`--scheduler`** — Explicitly enable the in-process schedule scanner

  **Default:** `false`
- **`--no-scheduler`** — Disable the in-process schedule scanner

  **Default:** `false`
- **`--worker-actor-filter <WORKER_ACTOR_FILTER>`** — Only claim jobs whose actor matches this tag
- **`--worker-script-filter <WORKER_SCRIPT_FILTER>`** — Only claim jobs whose script path or name contains this pattern
- **`--readiness-requires-worker`** — Fail `/v1/ready` when configured workers are not alive

  **Default:** `false`
- **`--readiness-requires-scheduler`** — Fail `/v1/ready` when the scheduler is enabled but not alive

  **Default:** `false`
- **`--readiness-requires-transport`** — Fail `/v1/ready` while configured static peers are not connected

  **Default:** `false`
- **`--tokens-file <TOKENS_FILE>`** — Multi-token TOML file. Same as `omakure api --tokens-file`
- **`--capability <CAPABILITIES>…`** — API capability to grant in legacy single-token mode. Repeatable
- **`--secret-ref <SECRET_REFS>…`** — Allowed secret provider ref for secrets:use. Same as `omakure api --secret-ref`
- **`--bootstrap-token-file <BOOTSTRAP_TOKEN_FILE>`** — Node-local one-time bootstrap token file for the signed-bundle API

## `omakure node direct-probe`

- **Usage:** `omakure node direct-probe <FLAGS>`

Establish a direct encrypted probe with one explicitly trusted peer

### Flags
- **`--endpoint <ENDPOINT>`** — Peer direct transport address
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--peer-node-id <PEER_NODE_ID>`** — Expected canonical peer node ID
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node cue`

- **Usage:** `omakure node cue <FLAGS>`

Ask one trusted Performer to run a script it has already declared

### Flags
- **`--endpoint <ENDPOINT>`** — Peer direct transport address
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--peer-node-id <PEER_NODE_ID>`** — Expected canonical peer node ID
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--script <SCRIPT>`** — Script name as the Performer declared it. A path is not accepted: the Performer resolves the name against what it published, and a Cue never carries a location
- **`--reason <REASON>`** — Why this is being asked for. Recorded in the Performer's audit trail
- **`--wait-seconds <WAIT_SECONDS>`** — How long to stay on the session waiting for the `run-completed` Signal.

  The outcome is read on the connection this dial already opened, because a Performer that holds a standing session with this Conductor refuses the dial outright — the configuration that would deliver the Signal is the one in which the Cue could not be sent. `0` dispatches and returns immediately; the run still happens and still reports.

  **Default:** `120`
- **`--direct`** — Dial the peer from this process instead of asking the running service.

  The service is preferred because it is the only thing that can reach a peer this node already has a session with. Use this for a peer there is no standing session with, or when no service is running.

  **Default:** `false`

## `omakure node baseline`

- **Usage:** `omakure node baseline [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Publish and deliver the signed set of scripts a fleet runs

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node baseline create-key`

- **Usage:** `omakure node baseline create-key [--scripts-dir <SCRIPTS_DIR>] [--json]`

Create this node's baseline publisher key, refusing to replace one

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node baseline publish`

- **Usage:** `omakure node baseline publish <FLAGS>`

Sign the named workspace scripts as one baseline

### Flags
- **`--script <PATH>…`** — A workspace-relative script path to include. Repeatable.

  A path rather than a bare name, because a baseline says *where* each script goes on every receiver; a name would leave that to whatever the receiver happened to guess.
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--lifetime-seconds <LIFETIME_SECONDS>`** — How long the baseline stays installable, in seconds

  **Default:** `3600`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--out <PATH>`** — Where to write the signed manifest

## `omakure node baseline push`

- **Usage:** `omakure node baseline push <FLAGS>`

Deliver a signed baseline to one trusted Performer

### Flags
- **`--peer-node-id <PEER_NODE_ID>`** — Expected canonical peer node ID
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--manifest <PATH>`** — The signed manifest produced by `node baseline publish`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--wait-seconds <WAIT_SECONDS>`** — How long to wait on the session for the Performer's answer

  **Default:** `120`

## `omakure node baseline rollback`

- **Usage:** `omakure node baseline rollback [FLAGS]`

Put this node back on the one baseline retained before the current one

A local operator action, not something a Conductor orders. The baseline plane carries exactly two message kinds and neither of them is "run the other version"; a remote rollback verb would hand a Conductor the power to flip a Performer between two code versions at will, which is a power the split between publishing and conducting exists to withhold. The drift status on `node health` says which machine to go and fix.

Exactly one previous baseline is retained, and this is a swap: rolling back twice returns this node to where it started. The retained set is re-verified against the publishers this node names *today*, so a publisher revoked since the original install makes the rollback fail.

### Flags
- **`--confirmed`** — Required. A rollback replaces every script the current baseline named

  **Default:** `false`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node init`

- **Usage:** `omakure node init [--scripts-dir <SCRIPTS_DIR>] [--json]`

Explicitly initialize public config, identity, and local trust state

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node status`

- **Usage:** `omakure node status [--scripts-dir <SCRIPTS_DIR>] [--json]`

Inspect public node identity, redacted config, and bounded trust counts

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node peers`

- **Usage:** `omakure node peers [--scripts-dir <SCRIPTS_DIR>] [--json]`

List registered peers without audit history or private state

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node health`

- **Usage:** `omakure node health [--scripts-dir <SCRIPTS_DIR>] [--json]`

Show current fleet health: presence, profile, and runner status

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node signals`

- **Usage:** `omakure node signals [--scripts-dir <SCRIPTS_DIR>] [--json]`

Show the bounded newest-first closed Signal feed: enrolled, revoked, run-completed

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node discovery`

- **Usage:** `omakure node discovery [FLAGS]`

Run one bounded in-memory LAN discovery scan

### Flags
- **`--wait-seconds <WAIT_SECONDS>`** — Discovery scan duration in seconds, bounded to 1..=30

  **Default:** `5`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--include-addresses`** — Include observed source addresses in the local CLI result

  **Default:** `false`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node trust`

- **Usage:** `omakure node trust <FLAGS>`

Explicitly import and activate one manually trusted peer

### Flags
- **`--node-id <NODE_ID>`** — Canonical omk1_ node identifier
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--public-key <PUBLIC_KEY>`** — Lowercase hexadecimal x-only BIP-340 public key
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--transport-certificate <TRANSPORT_CERTIFICATE>`** — Signed transport certificate as lowercase hexadecimal bytes
- **`--role <ROLE>`** — Peer role: conductor or performer

  **Default:** `performer`
- **`--capability <CAPABILITIES>…`** — Allowed capability (repeatable; sorted unique values are required)
- **`--actor <ACTOR>`** — Audit actor
- **`--reason <REASON>`** — Audit reason/evidence
- **`--confirmed`** — Confirm this trust mutation explicitly

  **Default:** `false`

## `omakure node enroll`

- **Usage:** `omakure node enroll [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Request and explicitly approve or reject manual enrollment

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node enroll request`

- **Usage:** `omakure node enroll request <FLAGS>`

Create and send one signed manual enrollment request

### Flags
- **`--endpoint <ENDPOINT>`** — Peer direct transport address
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--role <ROLE>`** — Requested peer role

  **Default:** `performer`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--capability <CAPABILITIES>…`** — Requested capability (repeatable; sorted unique values are required)
- **`--lifetime-seconds <LIFETIME_SECONDS>`** — Request lifetime in seconds, at most 30 days

  **Default:** `600`

## `omakure node enroll approve`

- **Usage:** `omakure node enroll approve <FLAGS>`

Approve one pending request after checking the out-of-band code

### Flags
- **`--request <REQUEST_HEX>`** — Exact signed OMMA request as lowercase hexadecimal bytes
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--transport-certificate <TRANSPORT_CERTIFICATE>`** — Candidate transport certificate as lowercase hexadecimal bytes
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--code <CODE>`** — Out-of-band 16-byte approval code as lowercase hexadecimal
- **`--actor <ACTOR>`** — Audit actor
- **`--reason <REASON>`** — Audit reason/evidence
- **`--confirmed`** — Confirm this trust mutation explicitly

  **Default:** `false`

## `omakure node enroll reject`

- **Usage:** `omakure node enroll reject <FLAGS> <NODE_ID>`

Reject one pending request without activating trust

### Arguments
- **`<NODE_ID>`** — Pending candidate node identifier

### Flags
- **`--actor <ACTOR>`** — Audit actor
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--reason <REASON>`** — Audit reason/evidence
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--confirmed`** — Confirm this denial explicitly

  **Default:** `false`

## `omakure node enroll apply`

- **Usage:** `omakure node enroll apply <FLAGS>`

Apply one authority-signed unattended enrollment bundle

### Flags
- **`--bundle-file <BUNDLE_FILE>`** — Exact signed OMEB bundle file. The file is never echoed or persisted
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--bootstrap-token-file <BOOTSTRAP_TOKEN_FILE>`** — One-time bootstrap token file. The token is never echoed or persisted
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--bootstrap-nonce <BOOTSTRAP_NONCE>`** — One-time 16-byte bootstrap nonce as lowercase hexadecimal

## `omakure node authority`

- **Usage:** `omakure node authority [--scripts-dir <SCRIPTS_DIR>] [--json] <SUBCOMMAND>`

Hold and use this node's enrollment authority

### Global Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node authority create`

- **Usage:** `omakure node authority create [FLAGS]`

Create this node's enrollment authority key, refusing to replace one

### Flags
- **`--confirmed`** — Required. Creating an authority is a fleet-wide act

  **Default:** `false`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node authority show`

- **Usage:** `omakure node authority show [--scripts-dir <SCRIPTS_DIR>] [--json]`

Report the authority this node holds, without its private half

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure node authority issue`

- **Usage:** `omakure node authority issue <FLAGS>`

Mint one enrollment bundle naming this node as the subject

### Flags
- **`--audience <AUDIENCE>`** — The node that will apply this bundle. It is checked against that node's own identity when it does, so a bundle is useless anywhere else
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--role <ROLE>`** — The role the audience will record for this node

  **Choices:** `conductor`, `performer`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--capability <CAPABILITIES>…`** — A capability the audience will grant this node. Repeatable
- **`--lifetime-seconds <LIFETIME_SECONDS>`** — How long the bundle stays valid, in seconds

  **Default:** `3600`

## `omakure node capabilities`

- **Usage:** `omakure node capabilities <FLAGS> <NODE_ID>`

Update one peer's capability allow-list with confirmation and evidence

### Arguments
- **`<NODE_ID>`** — Peer node identifier

### Flags
- **`--capability <CAPABILITIES>…`** — Allowed capability (repeatable; sorted unique values are required)
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--actor <ACTOR>`** — Audit actor
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--reason <REASON>`** — Audit reason/evidence
- **`--confirmed`** — Confirm this trust mutation explicitly

  **Default:** `false`

## `omakure node revoke`

- **Usage:** `omakure node revoke <FLAGS> <NODE_ID>`

Revoke one peer with confirmation and evidence

### Arguments
- **`<NODE_ID>`** — Peer node identifier

### Flags
- **`--actor <ACTOR>`** — Audit actor
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--reason <REASON>`** — Audit reason/evidence
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--confirmed`** — Confirm this trust mutation explicitly

  **Default:** `false`

## `omakure node reset`

- **Usage:** `omakure node reset [FLAGS]`

Explicitly remove validated machine identity and node trust state

### Flags
- **`--confirmed`** — Confirm destructive removal of identity and trust state

  **Default:** `false`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure config`

- **Usage:** `omakure config [--scripts-dir <SCRIPTS_DIR>] [--json]`

Show resolved paths and environment diagnostics

Prints the resolved binary path, omakure version, workspace root, scripts root, `.omakure/` directory, history directory, workspace config file, environments directory, active environment, and any known env overrides (`OMAKURE_SCRIPTS_DIR`, `OMAKURE_REPO`, `OVERTURE_*`, `CLOUD_MGMT_*`, `REPO`, `VERSION`). Pass `--json` for the machine-readable envelope.

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure update`

- **Usage:** `omakure update [FLAGS]`

Update omakure from GitHub releases

Downloads the release archive for the current OS/arch and replaces the running binary in place. Also copies any scripts missing from your local scripts directory from the source archive of the target version — existing files are never overwritten. `--repo` defaults to `$OMAKURE_REPO` / `$OVERTURE_REPO` / `$CLOUD_MGMT_REPO` / `$REPO` / `This-Is-NPC/omakure`; `--version` defaults to `$VERSION` or the latest GitHub release.

### Flags
- **`--repo <REPO>`** — GitHub repository (`owner/name`). Defaults to `$OMAKURE_REPO` / `$OVERTURE_REPO` / `$CLOUD_MGMT_REPO` / `$REPO` / `This-Is-NPC/omakure`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--version <VERSION>`** — Release tag to install (e.g. `v0.1.9`). Defaults to `$VERSION` or the latest GitHub release for the configured repo
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure uninstall`

- **Usage:** `omakure uninstall [FLAGS]`

Remove the omakure binary (optionally wipe the scripts workspace)

Deletes the currently running binary from its install directory (on Windows also strips the install path from the user `PATH`). With `--scripts`, PERMANENTLY deletes the entire scripts workspace, including `.omakure/` (envs, daemon files), `.history/`, schedules) and every script file — use with care and have backups.

### Flags
- **`--scripts`** — Also delete the scripts workspace directory (runs.sqlite, history, schedules, and every user script). Destructive

  **Default:** `false`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure completion`

- **Usage:** `omakure completion [--scripts-dir <SCRIPTS_DIR>] [--json] <SHELL>`

Generate shell completion script for the given shell

Writes the completion script to stdout. Quick install examples:
 bash: `omakure completion bash >> ~/.bashrc`
 zsh:  `omakure completion zsh  > ~/.zfunc/_omakure` (ensure `~/.zfunc` is on `$fpath`)
 fish: `omakure completion fish > ~/.config/fish/completions/omakure.fish`
 pwsh: `omakure completion pwsh | Out-String | Invoke-Expression`

For a one-shot session pipe into your current shell: `eval "$(omakure completion zsh)"`.

### Arguments
- **`<SHELL>`** — Shell to generate completions for

  **Choices:** `bash`, `zsh`, `fish`, `pwsh`

### Flags
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`

## `omakure serve`

- **Usage:** `omakure serve [FLAGS]`

Run the cron scheduler daemon for scripts declaring a `Schedule` block

Running `omakure serve` with no flags starts the scheduler in the foreground with an in-process worker; `-d`/`--detach` daemonizes (Unix) and `--stop` terminates a running daemon.

The scheduler rescans the workspace every 5 seconds, parses each script's `Schedule` block, and enqueues a run when the cron expression is due. Fires are SKIPPED when a previous run with the same `cron_schedule_id` is still `queued` or `running`, so long-lived overlapping jobs never stack up.

Paths (per workspace):
 PID file: `<workspace>/.omakure/daemon.pid`
 Log:      `<workspace>/.omakure/daemon.log`

`--install`/`--uninstall`/`--status` manage a per-workspace systemd user unit so the daemon survives reboots (Linux only); after install tail with `journalctl --user -u <unit> -f`.

By default an in-process worker is spawned so scheduled rows execute without a separate process. Pass `--no-worker` when you run `omakure queue worker` elsewhere.

### Flags
- **`-d --detach`** — Run the scheduler as a detached background daemon (Unix only)

  **Default:** `false`
- **`--scripts-dir <SCRIPTS_DIR>`** — Scripts directory override
- **`--stop`** — Stop a running daemon (reads `.omakure/daemon.pid` and sends SIGTERM)

  **Default:** `false`
- **`--json`** — Emit machine-readable JSON output for AI-facing subcommands.

  When set, supported subcommands print exactly one JSON envelope `{ ok, data, error, schema_version }` on stdout instead of their human-readable form. Subcommands that do not support JSON ignore this flag.

  **Default:** `false`
- **`--install`** — Install a systemd user service that runs `omakure serve` for the current workspace and survives reboots (Linux only)

  **Default:** `false`
- **`--uninstall`** — Disable and remove the systemd user service for the current workspace (Linux only)

  **Default:** `false`
- **`--status`** — Print the systemd user service status for the current workspace (Linux only)

  **Default:** `false`
- **`--no-worker`** — Do not spawn the in-process worker. Use when you already run `omakure queue worker` elsewhere

  **Default:** `false`
- **`--concurrency <CONCURRENCY>`** — Number of worker threads for the in-process worker (default 1)

  **Default:** `1`
