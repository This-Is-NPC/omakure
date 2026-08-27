# CLI and HTTP usage

Omakure has no interactive application mode. Running the binary without a
subcommand prints help; every operational invocation names a command.

## Discovery and workspace

```bash
omakure --help
omakure help-ai
omakure --json config
omakure --json doctor
omakure --json scripts
omakure --scripts-dir /path/to/workspace --json scripts
```

The workspace is selected by `--scripts-dir`, then `OMAKURE_SCRIPTS_DIR`, then
legacy environment overrides, then the debug `scripts/` fixture and platform
defaults. A positional path is not accepted and must not be reintroduced as a
headless alias.

## Scripts and runs

```bash
omakure --json scripts --tag ops --tag production
omakure --json describe tools/deploy.py
omakure --json search deploy --tag production
omakure init tools/job.sh
omakure --json init tools/job.sh --schema-json @schema.json --body-stdin < body.sh
omakure --json run tools/deploy.py --actor agent --reason rollout -- --target prod
omakure --json run tools/deploy.py --env-file ./prod.env --no-prompt -- --target prod
```

Schemas use PascalCase keys and comment markers. See
`how-to-create-a-script.md`. Supported script extensions are `.bash`, `.sh`,
`.ps1`, `.py`, and `.lua`. Bash, PowerShell, and Python need their interpreter
installed; `.lua` does not, because the Lua runtime is embedded in the binary.
Extensionless names resolve in that order, so `omakure run deploy` picks
`deploy.sh` over `deploy.lua`.

## Queue and history

```bash
omakure --json queue add tools/deploy.py --actor agent --priority 10 -- --target prod
omakure queue worker --concurrency 4
omakure --json queue cancel RUN_ID --reason "superseded"
omakure --json queue dead-letter RUN_ID --reason "needs investigation"
omakure --json queue stats
omakure --json history list --state-set all --limit 20
omakure --json history show RUN_ID
omakure --json history stats
omakure --json history traces RUN_ID --since-sequence 0
```

Workers reclaim expired leases. Direct, queued, and scheduled runs share the
same executor, timeout, cancellation, redaction, and SQLite state machine.

## Environments

```bash
omakure env list
omakure env create prod HOST=prod.example.com API_KEY=secret://prod/api_key
omakure env show prod
omakure env set prod REGION=eastus
omakure env replace prod HOST=prod.example.com REGION=eastus
omakure env activate prod
omakure env deactivate
omakure env delete prod
```

Managed files live under `.omakure/envs/`. Active values are injected into
child processes; `--env-file` overrides them for one run and reserved Omakure
variables win last. Sensitive values are masked in diagnostics and excluded
from run storage. See `environments.md` and `env-injection-spec.md`.

## HTTP API and node service

API-only mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --capability all
```

The recommended single-process deployment is:

```bash
omakure token generate --id local --scope '*' --json
omakure node serve --workers 1 --tokens-file /run/secrets/omakure_tokens.toml
```

The default bind is `127.0.0.1:7878`; non-loopback binding requires
`--allow-non-loopback`. Prefer tokens-file auth with per-token scopes. Legacy
`OMAKURE_API_TOKEN` plus `--capability` remains available for local migration.

Unauthenticated probes:

```bash
curl http://127.0.0.1:7878/v1/health
curl http://127.0.0.1:7878/v1/ready
```

All other routes require `Authorization: Bearer <token>`. See `http-api.md`
for routes and `deployment.md` for policy and container operation.

## Remote Cues

A Cue asks a trusted Performer to run a script that Performer already declared.
It names a script and never carries one, so remote management can select among
code a node already has and can never introduce more.

Default off, and off in two independent places. The Performer opts in:

```toml
[trust]
enrollment = "manual"          # remote capabilities require enrollment enabled
allow_remote_cues = true
remote_cue_scripts = ["deploy.sh"]
remote_cue_batteries = []
```

An empty `remote_cue_scripts` with `remote_cue_batteries` empty denies
everything, even with `allow_remote_cues = true`. Declaring a battery grants the
scripts that battery installed, read from the local install record.

The Conductor dispatches:

```bash
omakure node cue --endpoint 127.0.0.1:7879 --peer-node-id omk1_… \
  --script deploy.sh --reason "roll out 1.4.2"
```

The reply reports `answered`, `accepted`, and `code` separately. A Performer
that refuses on trust, role, or capability says nothing at all, so `answered:
false` is a legitimate answer rather than an error — an unauthorized sender does
not learn that the feature exists.

The outcome arrives as an ordinary `run-completed` Signal, correlated by a run
id both sides derive from the Cue id; no message carries a correlation field.
`expected_run_id` in the reply is what to match in `omakure node signals`, and
`--wait-seconds` (default 120, `0` to return immediately) bounds how long the
command waits for it.

**Which session carries it.** A node holds one session per peer, so a separate
process cannot dial a peer the running service is already connected to — the
normal state of a managed fleet. `omakure node cue` therefore asks the running
service to send it, over `POST /v1/node/cues` under the same `node:write` scope
as the rest of the node surface, and falls back to dialling directly only when
no service is listening or there is no session with that peer. `via` in the reply
says which path was taken; `--direct` forces the dial.

Cue-origin runs execute with an explicit deny-all secret policy, and a script
whose schema declares a secret field is refused at the gate rather than run
without its secrets. See `remote-cue-contract.md`.

## Fleet health

An enrolled Performer reports a Profile on connect or material change and a
Pulse on a bounded cadence, over the same authenticated direct transport it
already uses. Reporting needs no flag: it starts when `node serve` has a peer
trusted in the `conductor` role with the `inventory-health` capability, and it
stops the moment that trust is revoked or narrowed.

On the Conductor, one bounded view answers "which of my nodes are up":

```bash
omakure node health --json
curl -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/node/health
```

Both render the same operation, so they always agree. Each actively trusted
peer appears once with its presence (`unknown` before it has ever reported,
then `online`, `stale`, or `offline`), its platform and runtime facts, its
runner and scheduler state, and its last completed run.

This is current status only. There is no chart, alert rule, host inventory,
raw log, or history API, and no HTTP call can write health state: the only
writer is the node-to-node exchange. `.docs/health-plane-contract.md` holds the
frozen windows, bounds, error codes, and privacy classes.

## Lifecycle Signals

Alongside current status, a Conductor keeps a small closed feed of notable
lifecycle changes:

```bash
omakure node signals --json
curl -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/node/signals
```

There are exactly three Signal kinds and no way to add a fourth:

- `enrolled` and `revoked` are decided by the Conductor itself and are read
  straight from its authoritative append-only trust log, so a revocation and
  its Signal can never disagree and the revocation Signal survives the
  revocation it records.
- `run-completed` is emitted by a Performer *after* one of its runs reaches a
  terminal result, and travels over the same authenticated direct transport.
  It needs the `notifications` capability: a Performer whose Conductor granted
  `inventory-health` but not `notifications` reports Profile and Pulse and
  sends no Signals at all.

A `run-completed` Signal carries only the run's schema name, an opaque run id,
the finish time, the terminal state, and the exit code. Script paths,
arguments, environment values, stdout, stderr, and secret references are
forbidden by the closed schema and are rejected rather than redacted.

Delivery is bounded, durable, and idempotent. Each Signal has a stable id, so
a duplicate, a reconnect, a restart, or a lost acknowledgement still produces
exactly one visible Signal. An undelivered Signal is retried at most three
times per session and is then kept in the queue and resent on the next
session, so a Conductor restart or a brief partition delays a Signal rather
than losing it. The feed is newest first, capped at 64 entries,
kept for seven days, and stalls rather than admitting a hole if a Signal goes
missing - `gap` and the per-node `cursor` in the response say so explicitly.
There are no subscriptions, webhooks, alerts, or user-defined Signal kinds.

## Transport certification

The repository's bounded Linux certification uses four isolated Compose services
and production node listeners. It covers encrypted direct probes, malformed,
oversized, downgraded, expired, forged, identity-mismatched, wrong-target,
untrusted, and exact-replay ingress, durable redacted audits with unchanged
registry-state snapshots, static-peer locator validation, partition/reconnect,
revocation, identity reset/replacement, and the retained discovery/manual/
signed-bundle suites.

```bash
mise run transport-certification
```

This is a development and CI gate, not a general fleet launcher. Nostr, remote
campaigns and MDM are outside its scope.

## Health Plane certification

The Health Plane has its own bounded Linux gate over four independently stateful
packaged nodes on one dedicated network — one Conductor, two Performers, and an
untrusted adversary:

```bash
mise run health-plane-certification
```

It proves Profile and Pulse over production Noise, fleet aggregation through
both `omakure node health --json` and `GET /v1/node/health`, all three Signal
kinds including one idempotent `run-completed` Signal from a real manual
`omakure run`, `online` → `stale` → recovery across a real network partition,
Health Plane persistence across a Conductor restart, immediate exclusion after
revocation plus the bounded retention purge, identity replacement, corrupt-row
quarantine and recovery, and the frozen retry budget over one continuously
connected session.

The adversarial matrix is injected over real production Noise sessions and
covers wrong target (1104), future (1109), stale (1108), unknown field (1114),
malformed (1102), oversized (1103), forged signature (1102), spoofed sender
(1102), cross-session binding (1110), replayed `message_id` (1110), non-increasing
Profile revision and Pulse sequence (1110), reordering past the buffer (1111),
inbox overflow (1113), flood (1112), missing capability (1106), wrong role
(1105), and revocation on a live session (1107), plus corrupt stored state
(1115). Each case asserts the exact stable code and the frozen reply policy.
Durable redacted audit rows are asserted for the cases the contract requires
them for, and trust and health state are asserted unchanged across the matrix
as a whole rather than after every individual case.

Management HTTP binds loopback inside each container and is never published, so
it cannot be the node-to-node data path; the gate asserts that directly. Nostr,
baselines, dashboards, alerting, arbitrary metrics, and MDM
are all outside its scope.

## Scheduling and local lifecycle

```bash
omakure serve
omakure serve --detach
omakure serve --stop
omakure serve --install
omakure serve --status
omakure serve --uninstall
```

`node serve` enables the scheduler by default; use `--no-scheduler` for API-only
or worker-only test fixtures. See `scheduling.md` for cron and overlap rules.

## Batteries, tokens, and lifecycle

```bash
omakure battery list
omakure battery add https://example.invalid/automation.git --name automation
omakure battery sync automation
omakure battery inspect automation
omakure battery scripts automation
omakure battery install automation tools/job.sh
omakure token generate --id ci --scope runs:enqueue --scope runs:read
omakure completion bash
omakure update --version vX.Y.Z
omakure uninstall
```

Cached Battery repositories are untrusted and never run directly. `update` and
`uninstall` are local lifecycle operations and are not HTTP routes.
