# AI Interface

This document is the contract for AI agents (LLMs, coding assistants,
autonomous agents) using `omakure` as a workflow runner. The CLI is the
contract — every AI-relevant verb produces a stable JSON envelope on
stdout when called with `--json`, and the run history is persisted in a
queryable SQLite database.

## Trust model

**The AI is a full user. Everything is audited.**

- No allowlists, no denylists, no per-script confirmation gates.
- No interactive prompts when invoked in AI mode.
- Every invocation is recorded in `<workspace>/.history/runs.sqlite`
  with `actor`, optional `reason`, the resolved script path, full
  argv with secret field values redacted, exit code, redacted stdout/stderr,
  start/end timestamps, and a stable `run_id` the agent can quote back later.

The user remains the gatekeeper at the OS level (file permissions, sudo,
network). Omakure does not attempt to sandbox what the underlying script
does.

## Secret handling

Script schemas may declare `Type: "secret"` fields. Secret fields are accepted
by `omakure describe`, `omakure init --schema-json`, CLI runs, queue runs, and
HTTP enqueue. `Choices` is not supported on secret fields.

Secret value resolution for a run checks, in order:

1. Direct secret field input (`omakure run --secret FIELD=VALUE`, or HTTP
   `POST /v1/runs` `secret_fields` when the value is a reconstructable
   `secret://...` reference for the queued worker).
2. Forwarded script args using the field's `Arg` or `--<Name>` flag. For HTTP
   queued runs, secret-field args must be reconstructable `secret://...` refs.
3. The merged run environment, including the active managed env and any
   per-run env selection.
4. The schema field `Default`.

Plain secret values are passed to the child process so the script can use them,
but stored run args use `<redacted>`. HTTP queued `secret_fields` and forwarded
secret-field args reject plaintext values because queued workers cannot
reconstruct them without storing the plaintext. If the supplied value is a `secret://...`
reference, Omakure resolves it before execution and stores the provider
reference rather than the resolved plaintext. Supported reference forms are
`secret://env/NAME`, legacy `secret://env:NAME`, and `secret://provider/key`;
non-`env` providers read `<workspace>/.omakure/envs/<provider>.conf` and resolve
`key` from that file.

During execution, Omakure writes resolved plaintext secrets to a short-lived
0600 redaction file and injects only `OMAKURE_REDACT_SECRETS_FILE` into the
child. `omakure trace` reads that file so script-emitted trace messages are
redacted before persistence; `OMAKURE_REDACT_SECRETS` is retained only as a
legacy trace fallback. Run output redaction removes secret values from captured
stdout/stderr in plain, JSON-escaped, slash-escaped, and URL-encoded forms.
Environment values and direct secret values are not persisted as separate
records in `runs.sqlite`; residual OS exposure remains for explicit process
argv/env values while the child is running.

## Storage privacy contract

`<workspace>/.history/runs.sqlite` is **private internal storage of the
omakure CLI**. Scripts, agents, and orchestrators must never open this
file directly — neither for reads nor for writes. The only legitimate
access paths are the documented verbs:

- writes: `omakure run`, `omakure queue add|cancel|dead-letter`,
  `omakure queue worker`, `omakure trace`
- reads: `omakure history list|show|stats|traces`, `omakure queue stats`

This is an architectural rule, not a soft convention. Every code path
in the omakure binary that touches `runs.sqlite` lives in `src/runs.rs`
and is the only writer in the codebase. Scripts launched by
`omakure run` or `omakure queue worker` reach the database **only** by
re-executing the omakure binary (typically via `omakure trace`), which
gives the omakure CLI full control over what is written and when.

### Why this matters

The trust model declares that the AI is a full user at the OS level.
The storage privacy contract is the **complementary** rule that bounds
the AI's audit surface: the AI cannot tamper with its own audit log
because the audit log is not part of the AI's accessible state.

Concretely:

- An agent cannot rewrite history to hide a failed run.
- An agent cannot fabricate a run that did not happen.
- An agent cannot read sibling agents' captured stdout/stderr through
  raw SQL queries that bypass the documented filters.
- An agent cannot corrupt the schema or the SQLite WAL pages by
  partial writes from a buggy script.
- Tools that wrap the omakure CLI for orchestration purposes (workflow
  engines, dashboards, custom dispatchers) get a single, stable contract
  via the verbs — they never have to worry about schema drift in
  `runs.sqlite`.

### Pipeline state lives elsewhere

If a pipeline needs persistent state of its own — migration history,
catalogues of visited URLs, intermediate artifacts, queues of work
local to one pipeline, application data — that state lives in a
**separate** datastore provisioned by the orchestrator (a
Postgres/MySQL/SQLite of the orchestrator's choice, exposed via env
vars or a sidecar). It does **not** live in `runs.sqlite`. The
omakure database has exactly one purpose: recording omakure's runs and
traces. Mixing pipeline data into it conflates two ownerships and
breaks the privacy contract for no benefit.

### Enforcing the contract at the OS level

For deployments where the agent is sandboxed in a container or
restricted user account (the canonical pattern for orchestrating an
AI agent fleet), the privacy contract can be backed by
**kernel-enforced isolation**, not just by documentation. Hardened
deployment pattern:

1. Mount `<workspace>/.history/` into the sandbox so the omakure
   binary can reach it, but make it owned by a UID **other than** the
   sandbox's runtime UID, with mode `700`.
2. Install the omakure binary inside the sandbox image with the
   `cap_dac_override+ep` file capability:

   ```dockerfile
   COPY --chown=root:root --chmod=755 omakure /usr/local/bin/omakure
   RUN setcap cap_dac_override+ep /usr/local/bin/omakure
   ```

3. Run the sandbox with `--security-opt no-new-privileges`. File
   capabilities survive `no-new-privileges`; setuid does not, which
   is why file capabilities are the recommended mechanism here.
4. Do **not** include a SQLite client in the sandbox image (`sqlite3`
   binary, language SQLite bindings reachable from the agent's
   runtime, etc.). Defense in depth: a buggy or compromised agent has
   no off-the-shelf tooling to bypass the omakure CLI even if the
   filesystem isolation has a gap.

With this layout, the sandbox UID has no read or write access to the
SQLite file. The omakure binary, when invoked, gains
`CAP_DAC_OVERRIDE` from its file capability and can open the file.
Children spawned by omakure (the actual script processes) do **not**
inherit the capability — they run as the sandbox UID, with no DAC
privilege, and therefore cannot touch `runs.sqlite` directly. The
script's only path back to omakure is `execve` of the omakure binary,
typically via `omakure trace`, which gets its own capability from the
filesystem attribute on each invocation.

This is the trust boundary: anything that wants to write the audit
log must `execve` the omakure binary, which means it goes through
clap argument parsing, the typed `runs.rs` helpers, and the JSON
envelope contract. There is no "write a row directly" path.

## Destructive upgrade notice

Upgrading to this version of `omakure` triggers **two destructive
cleanups** on first launch against an existing workspace:

1. Every top-level `*.json` file in `<workspace>/.history/` is deleted
   (legacy per-run JSON history layout from pre-v0.1 releases).
2. If `<workspace>/.history/runs.sqlite` exists with the v0.1 schema
   (i.e. the `runs` table has no `state` column), the table is
   **dropped and recreated** with the new state-machine schema. Every
   row in the legacy table is lost.

Both cleanups are intentionally narrow:

- only top-level files in `history_dir()` are touched by the JSON cleanup
- only files whose extension is exactly `.json`
- subdirectories and `search-index.sqlite`
  are left untouched
- the schema rebuild only drops and recreates the `runs` and
  `run_traces` tables — not the database file or any other table

If you care about historical run data from older releases, **back up
`<workspace>/.history/` before upgrading**.

## JSON envelope

Every `--json` payload uses the same envelope shape:

```json
{
  "ok": true,
  "data": <payload>,
  "error": null,
  "schema_version": "1"
}
```

On failure:

```json
{
  "ok": false,
  "data": null,
  "error": {
    "code": "<stable-string>",
    "message": "<human readable>"
  },
  "schema_version": "1"
}
```

`schema_version` is bumped only when the envelope or any documented
`data` shape changes in a non-backward-compatible way.

### Stable error codes

| Code                       | Meaning                                                   |
|----------------------------|-----------------------------------------------------------|
| `not_found`                | Script, run, or other resource does not exist             |
| `schema_invalid`           | Embedded schema does not parse                            |
| `script_exists`            | `omakure init` would overwrite without `--force`          |
| `missing_required_field`   | `run --no-prompt` and a required field has no `--<flag>`  |
| `invalid_argument`         | Argument value cannot be parsed (e.g. bad `--since`)      |
| `not_implemented`          | Existing flag or platform path is intentionally unsupported |
| `internal`                 | Catch-all for I/O / SQLite / unexpected errors            |

## Verbs

The AI surface is the set of CLI commands that emit the stable JSON envelope
through `--json`, plus `help-ai` which is always JSON.

### `omakure help-ai`

Always JSON. Returns the full capability surface in one payload so the
agent can fetch it once at session start instead of re-discovering the
surface command-by-command. The verbs and flags list is generated by
walking clap's command tree, so it cannot drift from `--help`.

```bash
omakure help-ai
```

### `omakure scripts --json`

Lists every script in the workspace with absolute and relative paths,
schema name/description/tags, and a compact field summary.

```json
{
  "ok": true,
  "data": [
    {
      "absolute_path": "/abs/scripts/deploy.sh",
      "relative_path": "deploy.sh",
      "name": "deploy",
      "description": "Deploy the service",
      "tags": ["ops"],
      "field_count": 1,
      "schema_error": null
    }
  ],
  "error": null,
  "schema_version": "1"
}
```

Scripts whose schema cannot be parsed appear with `name: null` and a
populated `schema_error`. The list is not aborted by per-script failures.

### `omakure describe <script> --json`

Returns the full parsed schema for one script. Field resolution rules
match `omakure run` (relative path, absolute path, or bare name with
extension probing).

```json
{
  "ok": true,
  "data": {
    "absolute_path": "/abs/scripts/deploy.sh",
    "relative_path": "deploy.sh",
    "name": "deploy",
    "description": "Deploy the service",
    "tags": ["ops"],
    "fields": [
      {
        "name": "target",
        "prompt": "Target environment",
        "type": "string",
        "order": 1,
        "required": true,
        "arg": "--target",
        "default": null,
        "choices": ["dev", "prod"]
      }
    ]
  },
  "error": null,
  "schema_version": "1"
}
```

Errors:

- Missing script → `error.code = "not_found"`
- Malformed schema → `error.code = "schema_invalid"`

### `omakure search <query> --json`

Surfaces the SQLite-backed script index used by the CLI and HTTP search
operations. Returns the same per-script shape as `scripts --json` so
results pipe between the two commands without translating fields.

```bash
omakure search deploy --json
```

### `omakure init <path> --schema-json '<json>|@file' [--body-stdin] [--force]`

Non-interactive script creation. The supplied schema is validated before
the file is written, embedded between `OMAKURE_SCHEMA_START` /
`OMAKURE_SCHEMA_END` with the right comment prefix for the extension,
and (optionally) the body is read verbatim from stdin.

```bash
omakure --json init agent_made.sh \
    --schema-json '{"Name":"agent_made","Description":"created by ai","Fields":[{"Name":"target","Type":"string","Order":1,"Required":true,"Arg":"--target"}]}' \
    --body-stdin <<'BODY'
#!/usr/bin/env bash
set -e
echo "running with target=$2"
BODY
```

Errors:

- Existing script without `--force` → `error.code = "script_exists"`
- Bad schema → `error.code = "schema_invalid"`

The created script is immediately discoverable by `omakure scripts` and
`omakure describe`.

### `omakure run <script> [-- ...args] --json`

Runs a script and prints exactly one JSON envelope on completion. The
exit code matches the script's exit code, so agents can branch on both
the JSON `success`/`error` and the process exit status.

Flags:

- `--actor <tag>` — recorded in the history row (default: `human`)
- `--reason <text>` — optional free-form reason
- `--run-id <id>` — caller-provided id, otherwise one is generated
- `--parent-run-id <id>` — for chained agent workflows
- `--no-prompt` — fail fast on missing required fields. Implied by `--json`
- `--secret FIELD=VALUE` — direct input for schema fields with `Type: "secret"`;
  prefer provider refs where possible because shell history and process
  inspection can expose plaintext command-line arguments;
  stored args redact plaintext values
- `--json` — always print exactly one envelope on completion

```bash
omakure --json run deploy --actor ai --reason "rolling out config" -- --target prod
```

Successful payload:

```json
{
  "ok": true,
  "data": {
    "run_id": "1700000000000-12345-0",
    "script_path": "/abs/scripts/deploy.sh",
    "script_name": null,
    "args_json": "[\"--target\",\"prod\"]",
    "actor": "ai",
    "reason": "rolling out config",
    "started_at": 1700000000000,
    "finished_at": 1700000001234,
    "duration_ms": 1234,
    "exit_code": 0,
    "success": true,
    "stdout": "...",
    "stderr": "",
    "error": null,
    "parent_run_id": null,
    "omakure_version": "0.1.7"
  },
  "error": null,
  "schema_version": "1"
}
```

When `--no-prompt` is set (or implied by `--json`) and any required
field is missing, the command exits non-zero **without** writing a
history row:

```json
{
  "ok": false,
  "data": null,
  "error": {
    "code": "missing_required_field",
    "message": "required field `target` is missing: expected `--target` on the command line"
  },
  "schema_version": "1"
}
```

### `omakure history list|show|tail [--json]`

Queries the SQLite run log. `list --json` returns a compact array
(stdout/stderr stripped to keep payloads small) ordered by `started_at
DESC`; `show <run_id>` returns the full row including stdout/stderr.

Filters on `list`:

- `--script <name-or-path>` — substring match on script_path or
  script_name
- `--actor <tag>` — exact match
- `--since <duration>` / `--until <duration>` — relative durations like
  `30s`, `15m`, `2h`, `7d`
- `--success` / `--failure` — mutually exclusive
- `--limit <N>` — cap row count

```bash
omakure --json history list --actor ai --since 1d --limit 20
omakure --json history show 1700000000000-12345-0
omakure --json history tail --limit 5
```

`history tail --follow` is accepted as a flag but currently returns
`error.code = "not_implemented"`; `tail` is a snapshot command.

### `omakure config --json`

Returns the resolved workspace root, scripts root, history dir, envs
dir, active environment, and bootstrap mode in one envelope. Useful for
agents that need to understand which workspace a command will operate on.

## `run_id` format

`run_id` is a synthetic, sortable string of the form
`<unix_ms>-<pid>-<counter>`:

- `<unix_ms>` — milliseconds since the Unix epoch when the row was
  generated
- `<pid>` — process id of the binary that generated it
- `<counter>` — process-local atomic counter, monotonic within a process

Stability guarantees:

- Lexically sortable by start time within a process and (because of
  millisecond resolution + pid) effectively across processes too.
- Unique within a workspace: SQLite `PRIMARY KEY` enforces uniqueness;
  attempting to insert a duplicate id fails.
- The format is documented but not parsed by `omakure` itself — agents
  may treat the string as opaque if they prefer.

## Worked example

A fresh agent walking the loop:

```bash
# 1. Discover capabilities (one call, cached for the session)
omakure help-ai > /tmp/omakure-surface.json

# 2. List available scripts
omakure --json scripts > /tmp/scripts.json

# 3. Inspect one script's schema
omakure --json describe deploy

# 4. Create a new script the agent invented
omakure --json init my-task.sh \
    --schema-json '{"Name":"my_task","Fields":[{"Name":"target","Type":"string","Order":1,"Required":true,"Arg":"--target"}]}' \
    --body-stdin <<'BODY'
#!/usr/bin/env bash
echo "doing work for $2"
BODY

# 5. Run the new script under an AI actor
RUN=$(omakure --json run my-task --actor ai --reason "smoke" -- --target prod)
RUN_ID=$(echo "$RUN" | jq -r .data.run_id)

# 6. Look up the recorded run later
omakure --json history show "$RUN_ID"

# 7. List recent AI runs
omakure --json history list --actor ai --since 1h
```

## Concurrency

The run log uses SQLite WAL mode with a 500ms busy timeout (the same
PRAGMA setup as `search-index.sqlite`). Two `omakure run` invocations
against the same workspace can write concurrently without corrupting
the log.

## Run state machine

Every row in `runs.sqlite` carries a `state` column whose value is one of
the following seven strings. The set is final and small: there is no
`paused`, `retrying`, `scheduled`, `expired`, `zombie`, or `blocked`
state.

| State        | Meaning                                                          |
|--------------|------------------------------------------------------------------|
| `queued`     | Row exists, waiting for a worker to pick it up                   |
| `running`    | A worker (or `omakure run`) is executing the script              |
| `completed`  | Finished with `success = true`                                   |
| `failed`     | Finished with `success = false` (non-zero exit or runner error)  |
| `cancelled`  | Caller cancelled before/during execution                         |
| `timed_out`  | Worker killed the process for exceeding `--timeout`              |
| `dead_letter`| Promoted from `failed` or `timed_out` for human/agent review     |

Allowed transitions:

```
queued → running → completed
                 → failed
                 → cancelled (mid-execution)
                 → timed_out
queued → cancelled (before execution starts)
failed → dead_letter
timed_out → dead_letter
```

Any other transition is rejected by `runs.rs` and surfaces as
`error.code = "invalid_argument"` to the caller.

`omakure run` is a synchronous fast path: it inserts the row directly
in `state='running'` (skipping `queued`), drives the script through the
shared execution helper, and transitions to the right terminal state on
completion. In-progress `omakure run` invocations are visible to
`omakure history list --state running` immediately, even when no worker
daemon is active.

## Queue verbs

```bash
omakure queue add <script> [--actor X] [--reason Y] [--priority N] \
                  [--timeout 30m] [--parent-run-id ID] [-- ...args]
omakure queue cancel <run_id> [--reason Y]
omakure queue dead-letter <run_id> [--reason Y]
omakure queue worker [--concurrency N] [--actor-filter X] [--script-filter X]
omakure queue stats [--json]
```

### `omakure queue add`

Pushes a `queued` row onto `runs.sqlite` and prints the new run id.
Default `actor = "human"`, `priority = 0`, no timeout, no reason.
`--timeout` accepts the [`humantime`](https://docs.rs/humantime/) format
(`30s`, `5m`, `1h30m`, etc.) and is stored on the row as
`timeout_ms`. Without `--timeout`, the job has no execution limit.

Errors:

- Missing script → `error.code = "not_found"` (no row written).
- Bad `--timeout` → `error.code = "invalid_argument"`.

### `omakure queue cancel`

- Against a `queued` row: instant transition to `cancelled`.
- Against a `running` row: the worker's heartbeat detects the new state
  on its next tick (250 ms by default) and kills the script.
- Against any terminal row: `error.code = "invalid_argument"`.
- Unknown id: `error.code = "not_found"`.

### `omakure queue dead-letter`

Promotes a `failed` or `timed_out` row to `dead_letter`. Used when an
agent decides "this failure is chronic and needs human / deeper-agent
attention". Other states are rejected with
`error.code = "invalid_argument"`.

### `omakure queue worker`

Long-running daemon that drains the queue.

- `--concurrency N` (default 1): N parallel workers in N OS threads.
  Each thread claims and runs independently.
- `--actor-filter X` / `--script-filter X`: only claim jobs whose actor
  / script matches the filter. AND-combined.
- For each job: claim atomically (`UPDATE … RETURNING`, `queued →
  running`), spawn the script with `OMAKURE_RUN_ID` injected, refresh
  the heartbeat lease every 250 ms, kill on `timeout_ms` if set,
  transition to the right terminal state on exit.
- SIGINT/SIGTERM finishes the in-flight job and exits cleanly.
- If a worker dies hard (SIGKILL/OOM/crash), the lease (`HEARTBEAT_MS =
  60_000`) eventually expires and the next worker that polls steals the
  job. The stolen job restarts from claim — there is no resumption
  mid-run.
- The heartbeat is internal (60 s constant) and is **not** a job
  timeout. It only governs crash-recovery latency.

### `omakure queue stats`

Returns counts per state and per actor in one envelope.

## Visibility verbs (`history`)

```bash
omakure history list [--state queued|running|completed|failed|cancelled|timed_out|dead_letter] \
                     [--state-set in_flight|terminal|all] \
                     [--script X] [--actor X] [--since 1h] [--until 30m] \
                     [--success|--failure] [--limit N] [--json]
omakure history show <run_id> [--json]
omakure history stats [--json]
omakure history traces <run_id> [--json] [--level info|warn|error|debug] [--since-sequence N]
```

`--state` is repeatable (logical OR within the flag). `--state-set` is a
shorthand: `in_flight = {queued, running}`,
`terminal = {completed, failed, cancelled, timed_out, dead_letter}`,
`all = every state`. The two flags are mutually exclusive.

When neither `--state` nor `--state-set` is set, `history list` defaults
to `--state-set terminal` so v0.1 callers see no behavior change.

`history stats` returns counts per state and per actor in one envelope —
the same data as `queue stats` but exposed under the visibility surface
for fleet dashboards.

`history traces` reads the structured trace stream of one run (see
below). The reader is a snapshot, not a follow stream — agents poll
incrementally with `--since-sequence` to fetch only new entries.

## Structured traces (`omakure trace`)

Scripts emit structured trace events at known points in their execution
so agents can reconstruct what happened from a few KB of structured
records instead of a 500 KB stdout dump.

```bash
omakure trace "<message>" [--level info|warn|error|debug] [--data '<json>']
```

Designed to be called **from inside a script** that was launched by
`omakure run` or `omakure queue worker`. Both inject `OMAKURE_RUN_ID`
into the child environment so the verb knows which run to attach to.

Behavior:

- Reads `OMAKURE_RUN_ID` from the environment. If unset, exits 0 with a
  one-line stderr warning so the script can be tested in isolation
  outside omakure without breaking.
- Validates `--data` as JSON before writing (`error.code =
  "invalid_argument"` on bad JSON).
- Validates `--level` against the allowed set (`error.code =
  "invalid_argument"` on bad level).
- Inserts one row in `run_traces` with a monotonic per-run `sequence`,
  using a SQLite transaction so two concurrent calls cannot collide.

```bash
# Inside a script launched by omakure:
omakure trace "starting browser" --level info --data '{"target":"https://example.gov.br"}'
omakure trace "login submitted"  --level info
omakure trace "captcha failed"   --level error --data '{"attempt":3}'
```

Read them back from outside:

```bash
omakure --json history traces $RUN_ID
omakure --json history traces $RUN_ID --level warn
omakure --json history traces $RUN_ID --since-sequence 50
```

`run_traces` schema:

```sql
CREATE TABLE run_traces (
    trace_id INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    sequence INTEGER NOT NULL,
    level TEXT NOT NULL,
    message TEXT NOT NULL,
    data_json TEXT,
    FOREIGN KEY(run_id) REFERENCES runs(run_id) ON DELETE CASCADE
);
```

Deleting a run cascades to its traces (`PRAGMA foreign_keys = ON` is
set on every connection).

## Tag filtering

`omakure scripts` and `omakure search` accept a repeatable `--tag` flag
with **AND** semantics. A script must carry every supplied tag in its
embedded schema's `Tags` field. Matching is case-sensitive on the
literal string.

```bash
omakure scripts --json --tag prefeitura --tag sp
omakure search prefeitura --json --tag production
```

## Scheduler producer contract

`omakure serve` scans enabled script `Schedule` blocks and enqueues due runs
through the same run state machine used by `omakure queue add`. Scheduled rows
use `trigger = Scheduled`, `actor = scheduler`, `reason = "cron: <expr>"`, and
`cron_schedule_id = <canonical_path>@<cron_expr>`.

The `--cron-schedule-id` flag on `omakure queue add` records the same
provenance field for manual replay or external producers. Rows with a non-null
`cron_schedule_id` participate in the scheduler overlap check when the id
matches the scheduler's `<canonical_path>@<cron_expr>` format.

## Worked example: agent fleet pushing work

```bash
# 1. Agent SP pushes a job onto the queue
RUN=$(omakure --json queue add extract-curitiba --actor agent-sp \
       --priority 10 --timeout 30m -- --target https://curitiba.pr.gov.br)
RUN_ID=$(echo "$RUN" | jq -r .data.run_id)

# 2. A worker on the same machine drains the queue
omakure queue worker --concurrency 4 &

# 3. The script (called by the worker) emits structured traces
#    via `omakure trace "..." --data '...'`. OMAKURE_RUN_ID is
#    already in its environment.

# 4. Agent SP polls the row's state from a sidecar
omakure --json history show "$RUN_ID"

# 5. While the script runs, agent SP samples its trace stream
omakure --json history traces "$RUN_ID" --since-sequence 0

# 6. The script fails. Agent SP fetches only error-level traces:
omakure --json history traces "$RUN_ID" --level error

# 7. Agent SP decides this failure is chronic and promotes it
#    so a deeper agent (or human) takes a look:
omakure --json queue dead-letter "$RUN_ID" --reason "captcha solver broken"

# 8. Fleet dashboard pulls counts:
omakure --json history stats
```

## Current boundaries

- Automatic retry policies (`retrying` state, exponential backoff,
  retry limits). Manual re-enqueue is the only retry mechanism.
- Job dependencies / DAGs.
- Multi-host coordination.
- Embedded MCP server. CLI and HTTP are the implemented integration surfaces.
- Streaming `run --json` output. Long-running scripts still print live
  output when `--json` is **not** set.
- `history tail --follow` and trace tail / follow mode. Both are
  snapshot reads in v1.
- Helper libraries / SDKs for `omakure trace` in different languages.
  Scripts call the binary directly.
- Cross-workspace history aggregation.
- Localized output. The AI surface is English-only.
- A `paused`, `scheduled`, `expired`, `zombie`, `retry_pending`, or
  `blocked` state. The state set is final.
