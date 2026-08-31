# CLI and HTTP usage

Omakure has no interactive application mode. Running the binary without a
subcommand prints help; every operational invocation names a command.

## Terms

The words the rest of the documentation uses, and what they mean in the shipped
product.

| Term | Meaning |
|---|---|
| Node | One Omakure installation on one machine. |
| Mesh | The peer-to-peer network of nodes. There is no fixed server. |
| Conductor | A node trusted to manage other nodes. |
| Performer | A node managed by a Conductor. |
| Profile | The static facts a Performer reports: OS, architecture, runtimes, capabilities, and the baseline it installed. |
| Pulse | The periodic liveness and runner health a Performer reports. |
| Signal | One event from the closed set a node emits: `enrolled`, `revoked`, `run-completed`. |
| Cue | An instruction to run a script the Performer has already declared. A Cue names a script and never carries one. |
| Baseline | The signed, versioned set of scripts a fleet runs, and the only way code reaches a node. |

The role is a property of the trust record on each side, not of the machine: a
node is a Conductor to the peers it manages and a Performer to the peer that
manages it.

## Discovery and workspace

```bash
omakure --help
omakure help-ai
omakure --json config
omakure doctor
omakure --json scripts
omakure --scripts-dir /path/to/workspace --json scripts
```

The workspace is selected by `--scripts-dir`, then `OMAKURE_SCRIPTS_DIR`, then
legacy environment overrides, then the debug `scripts/workspace` fixture and
platform defaults. A positional path is not accepted and must not be
reintroduced as a headless alias.

`check` is the visible alias for the same workspace diagnostics:

```bash
omakure check
```

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

To supply a caller-owned correlation id, pass `--run-id`:

```bash
omakure --json queue add tools/deploy.py --run-id deploy-2026-08-30
```

The id is stored with the queued row and appears in worker/history output.

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
from run storage. See `environments.md` and `internal/env-injection-spec.md`.

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

The local CLI handoff used by `omakure node cue` and `omakure node baseline
push` does not read the tokens file. Set `OMAKURE_API_TOKEN` in that CLI process
to the plaintext value of a token whose tokens-file entry grants `node:write`.
The service can remain in tokens-file mode; the variable supplies the local
client with the bearer token it sends to the loopback API. Alternatively, call
`POST /v1/node/cues` or `POST /v1/node/baselines` directly with that token.

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
without its secrets. See `internal/remote-cue-contract.md`.

## Manual enrollment

The other way into a fleet, and the one that needs a human at both ends. It is
gated by `trust.enrollment = "manual"`; with the shipped `"disabled"` the
commands below are refused and the node keeps serving.

The joining node makes the offer, naming what it wants to be:

```bash
omakure node enroll request --endpoint 10.0.0.5:7879 \
  --role performer --capability inventory-health --lifetime-seconds 3600
```

That prints a `request_hex` and a **code**. The receiving node stores only the
code's hash and compares it in constant time, so holding the request bytes is
not enough to approve them — the code has to arrive by some other channel, which
is the point: it is what a person confirms out of band.

On the receiving node:

```bash
omakure node enroll approve --request <request_hex> \
  --transport-certificate <cert_hex> --code <code> \
  --actor ops --reason "new performer, ticket 412" --confirmed

omakure node enroll reject <node_id> \
  --actor ops --reason "unrecognised request" --confirmed
```

Both require `--confirmed`, an actor, and a reason, and both write an audit row.
A rejection activates no trust. Pending requests are listed by
`omakure node status` and over `GET /v1/node/enrollments`.

## Enrollment authority

A fleet needs something that can say "this node belongs to me". That is an
enrollment authority: a signing key held by one node, whose public half every
member names in `trust.authorities`.

```bash
omakure node authority create --confirmed
omakure node authority show
```

The key is **not** the node identity key. Both are BIP-340 scalars and sharing
one would cost nothing to implement, but it would mean compromising any single
node's identity hands over the right to mint membership for the whole fleet.
It lives beside the identity, under the same 0700 directory, written at 0600,
re-validated for owner and mode on every read, and returned by no read path.

`create` refuses to replace an existing key. Rotating an authority invalidates
every bundle it ever signed and every `trust.authorities` entry naming it, on
every machine in the fleet — that is a fleet-wide event, not something a
repeated command should do quietly.

Issuing names the node that will apply the bundle; the subject is always the
issuing node, because an authority issues membership in its own fleet:

```bash
omakure node authority issue --audience omk1_… --role conductor \
  --capability remote-run --lifetime-seconds 3600
```

The reply carries `bundle_hex` plus the two values the audience's `node.toml`
must already contain for it to be accepted at all — the authority `key_id` and
`public_key` — and the organization the bundle names. A bundle is checked
against the applying node's own identity, so it is useless anywhere else.

### How a provisioned machine joins

A bundle **cannot be pre-placed**. It names the node that will apply it, and is
checked against that node's own identity; the identity is generated from a
keypair the machine creates on first start. So there is nothing to ship in the
image that a fresh machine could apply to itself.

What the installer places is the public
`node.toml`, the authority's public key, the organization, both bootstrap
hashes, and the bootstrap token file. The machine boots, generates its identity,
and serves — belonging to nobody yet.

The fleet then issues membership for that identity and delivers it:

```bash
# on the machine holding the authority
omakure node authority issue --audience omk1_… --role conductor \
  --capability inventory-health --capability notifications

curl --fail -H 'Authorization: Bearer <target token>' \
  -H 'Content-Type: application/json' \
  --data '{"bundle_hex":"…","bootstrap_nonce":"…"}' \
  http://<target>:7878/v1/node/enrollment/bundle
```

Nothing is typed into the joining machine.

**Reachability.** The target's management API binds loopback by default, so
joining this way needs `--allow-non-loopback` and a token the fleet holds. Give
that token `enrollment:write` and nothing else — it is the narrowest scope that
admits the operation. It is not sufficient on its own: the bundle must still
verify against the authority the target's *own* config names, and the bootstrap
token and nonce must match the hashes its operator placed. Three independent
gates, and the token is only the first.

### What happens when a bundle cannot be used

Delivery is a request, so **every refusal is answered to the caller** with a
typed code — unknown or revoked authority, wrong organization, expired,
replayed, enrollment not enabled. The fleet that pushed is the fleet that reads
the answer.

That is worth stating plainly because an earlier draft of this section froze the
opposite shape. It described a node applying a bundle at boot and had to reason
about what a machine should do when "nobody is watching". Under delivery there
is no such moment: nothing is applied unprompted, and there is no unwatched
failure to design around. The rule that survives from it is the one that still
matters:

**Refusing enrollment must never mean refusing to serve.** A node that will not
boot cannot be asked what went wrong, and one bad bundle would take down a
rollout batch. A target that refuses a bundle keeps serving, and stays available
for the corrected one.

Two cases are deliberately not failures on the target:

| Case | Behaviour |
|---|---|
| Never enrolled | Serves normally. Most nodes are this, most of the time. |
| Already enrolled, or the bootstrap token already consumed | The delivery is refused to the caller; the node is unaffected. The token is consumed and tombstoned on first success, so treating its absence as a boot failure would let a node start exactly once. |

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

Each row also carries `baseline_status`, which is how a Conductor sees drift.
That is a comparison rather than a claim: a Performer reports the identity of
the set it recorded installing and the identity of the same set as it is on its
disk right now, and never a verdict, because it does not know what it was
supposed to have.

| `baseline_status` | What it means |
|---|---|
| `unknown` | This Performer has not reported a Profile yet. Not an answer about a baseline. |
| `none` | It reported holding no baseline. It was never pushed one, so it is neither drifted nor in sync. |
| `in_sync` | What is on its disk is the set it recorded installing. |
| `drifted` | It is not. |

The `baselines` object in the same response totals the four across the fleet.
`unknown` and `none` stay separate there too: ten machines that have never
reported and ten that hold no baseline need looking into in different places.

Drift is noticed within one nominal Pulse interval of a script changing. It
means "the set that was installed is no longer what was installed" — a file no
baseline entry names is not part of the published set and does not change the
answer.

This is current status only. There is no chart, alert rule, host inventory,
raw log, or history API, and no HTTP call can write health state: the only
writer is the node-to-node exchange. `docs/internal/health-plane-contract.md` holds the
frozen windows, bounds, error codes, and privacy classes.

## Baselines

A baseline is the signed, versioned set of scripts a fleet runs. Three
principals, and two of them may never be one machine: a **publisher** signs the
set, a **Conductor** delivers it, and a **Performer** installs it. The registry
refuses to let one node hold a publisher key and record a Performer, so nobody
can both author code and order every machine to run it.

```bash
# On the publisher, once. Rotating this key orphans every baseline it signed.
omakure node baseline create-key

# On the publisher, over scripts in its own workspace.
omakure node baseline publish --script ops/deploy.sh --script audit.py   --lifetime-seconds 3600 --out ./baseline-v1.omb

# On the Conductor, which holds the same script bytes and no publisher key.
omakure node baseline push --peer-node-id omk1_... --manifest ./baseline-v1.omb
curl -X POST -H "Authorization: Bearer $OMAKURE_API_TOKEN"   -H 'content-type: application/json'   --data '{"peer_node_id":"omk1_...","manifest":"<hex>","scripts":["<hex>"]}'   http://127.0.0.1:7878/v1/node/baselines
```

A Performer installs only if **both** authorities hold: the sender is an active
Conductor holding `baseline-push`, *and* the manifest verifies under a publisher
named in its own `trust.baseline_publishers`. It installs the whole set or none
of it. A node that names no publisher accepts no baseline, which is the shipped
state.

The version identifier is derived from the entry list alone — not from who
pushed it, not from when — which is what lets a Performer recompute it from the
files on its own disk and what makes drift checkable rather than merely
reported.

### Putting a machine back

Each node keeps exactly one previous baseline beside the one it is running:

```bash
omakure node baseline rollback --confirmed
curl -X POST -H "Authorization: Bearer $OMAKURE_API_TOKEN"   -H 'content-type: application/json' --data '{"confirmed":true}'   http://127.0.0.1:7878/v1/node/baseline/rollback
```

This is a local act on the machine that holds the scripts, on purpose: a
Conductor cannot order it. It is re-verified against the publishers that node
names *today*, so a publisher revoked since the original install makes it fail.
It is a swap rather than a step down a stack — rolling back twice returns the
machine to where it started — and a node with nothing retained refuses rather
than reporting a rollback that changed nothing.

`docs/internal/baseline-delivery.md` holds the wire format, the gate order, every bound,
and what "previous" means. `docs/recovery.md` walks a drifted machine back.

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

## Certification gates

Two bounded Linux gates run the product against itself over real Compose
topologies and production listeners, not mocks:

```bash
mise run cert:transport
mise run cert:health
```

The first certifies direct transport and enrolment; the second certifies the
Health Plane over four independently stateful nodes. What each one builds and
exactly which cases it covers is in `deployment.md`, under the two
certification-topology sections, so the coverage lists have one home rather
than two that drift.

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
or worker-only test fixtures. Scheduler and workers must use one host-local
workspace and its `.history/runs.sqlite`; this is not a cross-host queue. See
`scheduling.md` for cron and overlap rules.

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

For private HTTPS repositories, keep the credential out of the registry and
store only a provider reference with `--token-ref`:

```bash
omakure battery add https://example.invalid/private.git --name private \
  --token-ref secret://git/token
```

The configured secret provider must resolve the reference at sync time; a
missing or unauthorized reference fails before the repository is fetched.

Cached Battery repositories are untrusted and never run directly. Battery
installation is Unix-only and is a local act that an authenticated HTTP
operator may initiate, but a peer or Remote Cue never may. `update` and
`uninstall` are local lifecycle operations and are not HTTP routes.
## `omakure node direct-probe`

**Synopsis:** `omakure node direct-probe --endpoint HOST:PORT --peer-node-id omk1_...`

The node must already be initialized and the peer must be explicitly trusted;
this command performs a bounded encrypted transport probe and does not mutate
trust state. A successful probe prints the peer identity and authenticated
session result. An unreachable endpoint or mismatched `--peer-node-id` is the
principal failure and exits non-zero without changing the registry.

```bash
omakure node direct-probe --endpoint 127.0.0.1:7879 --peer-node-id omk1_example
```

Relevant flags are `--endpoint` and `--peer-node-id`. See the complete
metadata in the [generated CLI reference](cli-reference.md#node-direct-probe).

## `omakure node init`

**Synopsis:** `omakure node init`

Run this once on a new node after selecting its workspace. It creates the
public node configuration, machine identity, and local trust state; it refuses
to replace an existing identity. Success reports the canonical node ID and
initialized paths. The principal failure is an existing or invalid identity,
which leaves the existing state untouched.

```bash
omakure node init
```

There are no command-specific flags; use the global `--scripts-dir` and `--node-config` overrides when preparing a fixture. See the [generated CLI reference](cli-reference.md#node-init).

## `omakure node discovery`

**Synopsis:** `omakure node discovery [--wait-seconds SECONDS] [--include-addresses]`

Discovery requires a reachable local network and an initialized node. It
performs a bounded trust-neutral LAN scan and never writes a trust record.
Success prints observed candidate node IDs (and addresses when requested).
The principal failure is a timeout or malformed response; no candidate is
trusted as a side effect.

```bash
omakure node discovery --wait-seconds 5 --include-addresses
```

Relevant flags are `--wait-seconds` (bounded to 1..=30) and
`--include-addresses`. See the [generated CLI reference](cli-reference.md#node-discovery).

## `omakure node trust`

**Synopsis:** `omakure node trust --node-id ID --public-key HEX --actor ACTOR --reason TEXT --confirmed`

The operator must possess the peer's canonical identity, public key, and
transport certificate when one is required. This explicitly confirmed
mutation writes the local trust registry and audit evidence. Success reports
the trusted peer and role. The principal failure is invalid key/certificate
material or missing `--confirmed`; the mutation is rejected before state
changes.

```bash
omakure node trust --node-id omk1_peer --public-key HEX \
  --actor operator --reason "approved fleet member" --confirmed
```

Relevant flags include `--node-id`, `--public-key`, `--transport-certificate`,
`--role`, repeatable `--capability`, `--actor`, `--reason`, and `--confirmed`.
See the [generated CLI reference](cli-reference.md#node-trust).

## `omakure node capabilities`

**Synopsis:** `omakure node capabilities --node-id ID --capability CAP --actor ACTOR --reason TEXT --confirmed`

This requires an existing trusted peer and an operator decision. It replaces
that peer's sorted capability allow-list and appends audit evidence; it does
not enroll or revoke the peer. Success prints the resulting capabilities.
The principal failure is an unknown peer or omitted confirmation, which leaves
the current allow-list intact.

```bash
omakure node capabilities --node-id omk1_peer --capability inventory-health \
  --actor operator --reason "grant health reporting" --confirmed
```

Relevant flags are `--node-id`, repeatable `--capability`, `--actor`,
`--reason`, and `--confirmed`. See the [generated CLI reference](cli-reference.md#node-capabilities).

## `omakure completion`

**Synopsis:** `omakure completion SHELL`

Completion generation requires only the installed binary and a supported shell.
It writes a completion script to stdout and does not mutate workspace state.
Success is shell syntax suitable for the shell's completion directory. The
principal failure is an unsupported `SHELL` value; no file is written by the
command itself.

```bash
omakure completion bash >> ~/.bash_completion
```

`SHELL` is one of `bash`, `zsh`, `fish`, or `pwsh`; the positional value is
required. See the [generated CLI reference](cli-reference.md#completion).

`omakure queue add` also accepts `--run-id ID` when a caller needs a stable
correlation id, and `omakure battery add` accepts `--token-ref
secret://provider/key` for private HTTPS sources. The visible `check` alias
is equivalent to `omakure doctor`:

```bash
omakure queue add tools/deploy.py --run-id deploy-2026-08-30
omakure battery add https://example.invalid/private.git --name private \
  --token-ref secret://git/token
omakure check
```

These options and the alias are also recorded in the [generated CLI
reference](cli-reference.md).
