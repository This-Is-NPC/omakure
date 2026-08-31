# Fleet operations

This is the operator guide for Omakure's direct, trusted fleet workflows. Before
starting, install Omakure and choose a workspace; the [local usage
guide](usage.md) covers those prerequisites. This page then covers node
initialization, peer discovery, trust decisions, enrollment, approved work, and
fleet state.

The [direct transport and enrollment contract](internal/direct-transport-contract.md)
owns the authenticated wire and registry rules. The [Remote Cue
contract](internal/remote-cue-contract.md), [Health Plane
contract](internal/health-plane-contract.md), and [Baseline delivery
contract](internal/baseline-delivery.md) own exact payloads, refusal codes, and
quantitative bounds. This page keeps the task-first commands and the safety
decisions an operator must make.

## Terms

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

## Initialize a node
In command examples, replace every value in angle brackets with a value from
your deployment before running the command.

**Synopsis:** `omakure node init`

Run this once on a new node after selecting its workspace. It creates the public
node configuration, machine identity, and local trust state; it refuses to
replace an existing identity. Success reports the canonical node ID and
initialized paths. The principal failure is an existing or invalid identity,
which leaves the existing state untouched.

```bash
omakure node init
```

There are no command-specific flags; use the global `--scripts-dir` and
`--node-config` overrides when preparing a fixture. See the [generated CLI
reference](cli-reference.md#omakure-node-init).

## Discover peers

**Synopsis:** `omakure node discovery [--wait-seconds SECONDS] [--include-addresses]`

Discovery requires a reachable local network and an initialized node. It
performs a bounded trust-neutral LAN scan and never writes a trust record.
Success prints observed candidate node IDs (and addresses when requested). The
principal failure is a timeout or malformed response; no candidate is trusted
as a side effect.

```bash
omakure node discovery --wait-seconds 5 --include-addresses
```

Relevant flags are `--wait-seconds` (bounded to 1..=30) and
`--include-addresses`. See the [generated CLI reference](cli-reference.md#omakure-node-discovery)
and the [direct transport contract](internal/direct-transport-contract.md) for
beacon and transport boundaries.

## Probe a trusted peer

**Synopsis:** `omakure node direct-probe --endpoint HOST:PORT --peer-node-id <peer-node-id>`

The node must already be initialized and the peer must be explicitly trusted;
this command performs a bounded encrypted transport probe and does not mutate
trust state. A successful probe prints the peer identity and authenticated
session result. An unreachable endpoint or mismatched `--peer-node-id` is the
principal failure and exits non-zero without changing the registry.

```bash
omakure node direct-probe --endpoint 127.0.0.1:7879 --peer-node-id <peer-node-id>
```

Relevant flags are `--endpoint` and `--peer-node-id`. See the complete metadata
in the [generated CLI reference](cli-reference.md#omakure-node-direct-probe).

## Trust a peer and set capabilities
**Synopsis:** `omakure node trust ...` or `omakure node capabilities ...`

A discovery result or a successful probe is not trust. The operator must possess
the peer's canonical identity, public key, and transport certificate when one is
required. Trust is an explicitly confirmed mutation with audit evidence:

```bash
omakure node trust --node-id <peer-node-id> --public-key <peer-public-key> \
  --actor operator --reason "approved fleet member" --confirmed
```

The principal failure is invalid key or certificate material or missing
`--confirmed`; the mutation is rejected before state changes. Relevant flags
include `--node-id`, `--public-key`, `--transport-certificate`, `--role`,
repeatable `--capability`, `--actor`, `--reason`, and `--confirmed`. See the
[generated CLI reference](cli-reference.md#omakure-node-trust).

Capabilities are a separate operator decision on an existing trusted peer. The
command replaces that peer's sorted capability allow-list and appends audit
evidence; it does not enroll or revoke the peer. An unknown peer or omitted
confirmation leaves the current list intact:

```bash
omakure node capabilities --node-id <peer-node-id> --capability inventory-health \
  --actor operator --reason "grant health reporting" --confirmed
```

Relevant flags are `--node-id`, repeatable `--capability`, `--actor`, `--reason`,
and `--confirmed`. See the [generated CLI reference](cli-reference.md#omakure-node-capabilities).

A successful handshake, discovery beacon, node ID, or certificate never grants
trust or a capability. Authorization is read from the receiving node's local
registry.

## Remote Cues

A Cue asks a trusted Performer to run a script that Performer already declared.
It names a script and never carries one, so remote management can select among
code a node already has and can never introduce more.

Remote Cues are off by default and remain off in two independent places. The
Performer opts in:

```toml
[trust]
enrollment = "manual"          # remote capabilities require enrollment enabled
allow_remote_cues = true
remote_cue_scripts = ["deploy.sh"]
remote_cue_batteries = []
```

An empty `remote_cue_scripts` with `remote_cue_batteries` empty denies
everything, even with `allow_remote_cues = true`. Declaring a battery grants
the scripts that battery installed, read from the local install record.

The Conductor dispatches:

```bash
omakure node cue --endpoint 127.0.0.1:7879 --peer-node-id <peer-node-id> \
  --script deploy.sh --reason "roll out 1.4.2"
```

The reply reports `answered`, `accepted`, and `code` separately. A Performer
that refuses on trust, role, or capability says nothing at all, so
`answered: false` is a legitimate answer rather than an error—an unauthorized
sender does not learn that the feature exists.

The outcome arrives as an ordinary `run-completed` Signal, correlated by a run
id both sides derive from the Cue id; no message carries a correlation field.
`expected_run_id` in the reply is what to match in `omakure node signals`, and
`--wait-seconds` (default 120, `0` to return immediately) bounds how long the
command waits for it.

The local run id is derived from the Cue id, so a duplicate Cue is answered
from the existing run instead of starting a second one. A Cue-origin run left
`running` by a crash is never re-claimed or re-executed. See the [Remote Cue
contract](internal/remote-cue-contract.md#the-at-most-once-rule) for the
at-most-once rule.

**Which session carries it.** A node holds one session per peer, so a separate
process cannot dial a peer the running service is already connected to—the
normal state of a managed fleet. `omakure node cue` therefore asks the running
service to send it, over `POST /v1/node/cues` under the same `node:write` scope
as the rest of the node surface, and falls back to dialling directly only when
no service is listening or there is no session with that peer. `via` in the
reply says which path was taken; `--direct` forces the dial.
The separate CLI process does not read the service's `--tokens-file`. Set
`OMAKURE_API_TOKEN` to the matching plaintext token with at least `node:write`
before running `node cue` or `node baseline push`. Generate it once with
`omakure token generate --id node-operator --scope node:write --json`, then
store its `data.tokens_file_entry` in the service tokens file:

```bash
export OMAKURE_API_TOKEN='<node-write-token>'
```

Cue-origin runs execute with an explicit deny-all secret policy, and a script
whose schema declares a secret field is refused at the gate rather than run
without its secrets. See the [Remote Cue contract](internal/remote-cue-contract.md)
and the [generated CLI reference](cli-reference.md).

## Manual enrollment

Manual enrollment needs a human at both ends. It is gated by
`trust.enrollment = "manual"`; with the shipped `"disabled"` setting, these
commands are refused and the node keeps serving.

The joining node makes the offer, naming what it wants to be:

```bash
omakure node enroll request --endpoint 10.0.0.5:7879 \
  --role performer --capability inventory-health --lifetime-seconds 3600
```

That prints a `request_hex` and a **code**. The receiving node stores only the
code's hash and compares it in constant time, so holding the request bytes is
not enough to approve them—the code has to arrive by another channel, which is
the point: it is what a person confirms out of band.

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

A fleet needs something that can say “this node belongs to me”: an enrollment
authority is a signing key held by one node, whose public half every member
names in `trust.authorities`.

```bash
omakure node authority create --confirmed
omakure node authority show
```

The key is **not** the node identity key. Sharing one would mean compromising a
single node's identity hands over the right to mint membership for the whole
fleet. It lives beside the identity, under the same 0700 directory, written at
0600, re-validated for owner and mode on every read, and returned by no read
path.

`create` refuses to replace an existing key. Rotating an authority invalidates
every bundle it ever signed and every `trust.authorities` entry naming it on
every machine in the fleet. That is a fleet-wide event, not something a
repeated command should do quietly.

Issuing names the node that will apply the bundle; the subject is always the
issuing node, because an authority issues membership in its own fleet:

```bash
omakure node authority issue --audience <node-id> --role conductor \
  --capability remote-run --lifetime-seconds 3600
```

The reply carries `bundle_hex` plus the authority `key_id`, `public_key`, and
organization that the audience's `node.toml` must already contain. A bundle is
checked against the applying node's own identity, so it is useless anywhere
else.

### How a provisioned machine joins

A bundle **cannot be pre-placed**. It names the node that will apply it, and is
checked against that node's own identity; the identity is generated from a
keypair the machine creates on first start. There is nothing to ship in an
image that a fresh machine could apply to itself.

The shipped installers create a starter `node.toml` and, when machine-service
provisioning is requested, copy an existing hashed API tokens file supplied with
`--node-tokens-file`. They do not provision an enrollment authority, an
organization, a bootstrap token, or bootstrap hashes: the generated config has
empty values and enrollment disabled. Provision the node-local bootstrap secret
separately, then configure the target with the authority public key,
organization, and the domain-separated hashes of the token and nonce:

```toml
[organization]
id = "<organization-id>"

[trust]
enrollment = "signed-bundle"
authorities = [{ key_id = "<authority-key-id>", public_key = "<authority-public-key>", revoked = false }]
bootstrap_token_hash = "<bootstrap-token-hash>"
bootstrap_nonce_hash = "<bootstrap-nonce-hash>"
```

Generate and store the one-time values on the target (never in `node.toml` or
an image), then copy only the resulting hashes into that config:

```bash
export BOOTSTRAP_TOKEN="$(openssl rand -hex 32)"
export BOOTSTRAP_NONCE="$(openssl rand -hex 16)"
install -d -m 700 /run/secrets
printf %s "$BOOTSTRAP_TOKEN" >/run/secrets/bootstrap.token
chmod 600 /run/secrets/bootstrap.token
TOKEN_HASH="$({ printf 'omakure/bootstrap-token/v1\0'; printf %s "$BOOTSTRAP_TOKEN"; } | sha256sum | cut -d' ' -f1)"
NONCE_HASH="$({ printf 'omakure/bootstrap-nonce/v1\0'; printf %s "$BOOTSTRAP_NONCE" | xxd -r -p; } | sha256sum | cut -d' ' -f1)"
printf 'bootstrap_token_hash = "%s"\nbootstrap_nonce_hash = "%s"\n' "$TOKEN_HASH" "$NONCE_HASH"
```

The service must receive the token path through
`OMAKURE_BOOTSTRAP_TOKEN_FILE` or `node serve --bootstrap-token-file`; the
authenticated HTTP route receives only the bundle and nonce. See the
[installation enrollment procedure](installation.md#unattended-signed-bundle-enrollment)
for the file ownership and permission requirements.

The fleet then issues membership for that identity and delivers it:

```bash
# on the machine holding the authority
omakure node authority issue --audience <node-id> --role conductor \
  --capability inventory-health --capability notifications

curl --fail -H 'Authorization: Bearer <target-token>' \
  -H 'Content-Type: application/json' \
  --data '{"bundle_hex":"<bundle-hex>","bootstrap_nonce":"<bootstrap-nonce>"}' \
  http://<target>:7878/v1/node/enrollment/bundle
```

Nothing is typed into the joining machine.

**Reachability.** The target's management API binds loopback by default, so
joining this way needs `--allow-non-loopback` and a token the fleet holds. Give
that token `enrollment:write` and nothing else—the narrowest scope that admits
the operation. It is not sufficient on its own: the bundle must still verify
against the authority the target's own config names, and the bootstrap token and
nonce must match the hashes its operator placed. Three independent gates, and
the token is only the first.

### When a bundle cannot be used

Delivery is a request, so **every refusal is answered to the caller** with a
typed code—unknown or revoked authority, wrong organization, expired, replayed,
or enrollment not enabled. The fleet that pushed reads the answer.

Refusing enrollment must never mean refusing to serve. A target that refuses a
bundle keeps serving and stays available for the corrected one. A never-enrolled
node serves normally. If it is already enrolled, or the bootstrap token was
already consumed, delivery is refused to the caller and the node is unaffected.
The token is consumed and tombstoned on first success; its absence is not a boot
failure.

## Fleet health

An enrolled Performer reports a Profile on connect or material change and a
Pulse on a bounded cadence, over the same authenticated direct transport it
already uses. Reporting needs no flag: it starts when `node serve` has a peer
trusted in the `conductor` role with the `inventory-health` capability, and
stops the moment that trust is revoked or narrowed.

On the Conductor, one bounded view answers “which of my nodes are up”:

```bash
omakure node health --json
curl -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/node/health
```

Both render the same operation. Each actively trusted peer appears once with
its presence (`unknown` before it has ever reported, then `online`, `stale`, or
`offline`), platform and runtime facts, runner and scheduler state, and last
completed run.

Each row also carries `baseline_status`, which is how a Conductor sees drift:

| `baseline_status` | What it means |
|---|---|
| `unknown` | This Performer has not reported a Profile yet. |
| `none` | It reported holding no baseline. |
| `in_sync` | What is on its disk is the set it recorded installing. |
| `drifted` | It is not. |

The Performer reports the identity of the set it recorded installing and the
identity of that set as it is on disk now; it never sends a verdict, because it
does not know what it was supposed to have. This is current status only: there
is no chart, alert rule, host inventory, raw log, or history API, and no HTTP
call can write health state. See the [Health Plane contract](internal/health-plane-contract.md)
for windows, bounds, error codes, and privacy classes.

## Baselines

A Baseline is the signed, versioned set of scripts a fleet runs. Three
principals are involved, and two may never be one machine: a **Publisher** signs
the set, a **Conductor** delivers it, and a **Performer** installs it. The
registry refuses to let one node hold a Publisher key and record a Performer.
Before pushing, configure the Performer with the Publisher's `key_id` and
`public_key` from `baseline create-key`. Both values are required in
`trust.baseline_publishers`; `allow_baseline_push` is the independent gate:

```toml
[trust]
allow_baseline_push = true
baseline_publishers = [
  { key_id = "<publisher-key-id>", public_key = "<publisher-public-key>", revoked = false },
]
```

The Conductor must also have an active trusted-Performer record with the
`baseline-push` capability. A missing gate or publisher entry keeps the default
deny behavior, so the push is refused before installation.

```bash
# On the publisher, once. Rotating this key orphans every baseline it signed.
omakure node baseline create-key

# On the publisher, over scripts in its own workspace.
omakure node baseline publish --script ops/deploy.sh --script audit.py \
  --lifetime-seconds 3600 --out ./baseline-v1.omb

# On the Conductor, which holds the same script bytes and no publisher key.
omakure node baseline push --peer-node-id <peer-node-id> --manifest ./baseline-v1.omb
curl -X POST -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  -H 'content-type: application/json' \
  --data '{"peer_node_id":"<peer-node-id>","manifest":"<manifest-hex>","scripts":["<script-hex>"]}' \
  http://127.0.0.1:7878/v1/node/baselines
```

A Performer installs only if **both** authorities hold: the sender is an active
Conductor holding `baseline-push`, and the manifest verifies under a Publisher
named in its own `trust.baseline_publishers`. It installs the whole set or none
of it. A node that names no Publisher accepts no Baseline, which is the shipped
state.

The version identifier is derived from the entry list alone—not from who pushed
it or when—so a Performer can recompute it from files on disk and make drift
checkable rather than merely reported.

### Putting a machine back

Each node keeps exactly one previous Baseline beside the one it is running. A
rollback is local on the machine that holds the scripts; a Conductor cannot
order it:

```bash
omakure node baseline rollback --confirmed
curl -X POST -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  -H 'content-type: application/json' --data '{"confirmed":true}' \
  http://127.0.0.1:7878/v1/node/baseline/rollback
```

Rollback is re-verified against the Publishers that node names today, so a
Publisher revoked since the original install makes it fail. It is a swap rather
than a step down a stack—rolling back twice returns the machine to where it
started—and a node with nothing retained refuses rather than reporting a
rollback that changed nothing. See the [Baseline delivery contract](internal/baseline-delivery.md)
for the wire format, gate order, bounds, and retention rules, and
[Recovery](recovery.md) for a drifted machine walkthrough.

## Lifecycle Signals

Alongside current status, a Conductor keeps a small closed feed of notable
lifecycle changes:

```bash
omakure node signals --json
curl -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/node/signals
```

There are exactly three Signal kinds and no way to add a fourth:

- `enrolled` and `revoked` are decided by the Conductor itself and read straight
  from its authoritative append-only trust log, so a revocation and its Signal
  can never disagree and the revocation Signal survives the revocation it
  records.
- `run-completed` is emitted by a Performer *after* one of its runs reaches a
  terminal result and travels over the same authenticated direct transport. It
  needs the `notifications` capability: a Performer whose Conductor granted
  `inventory-health` but not `notifications` reports Profile and Pulse and
  sends no Signals at all.

A `run-completed` Signal carries only the run's schema name, an opaque run id,
the finish time, terminal state, and exit code. Script paths, arguments,
environment values, stdout, stderr, and secret references are forbidden by the
closed schema and rejected rather than redacted.

Delivery is bounded, durable, and idempotent. Each Signal has a stable id, so a
duplicate, reconnect, restart, or lost acknowledgement still produces exactly
one visible Signal. Undelivered Signals remain queued for a later session
rather than being silently lost. The feed is newest first, has bounded
retention, and stalls rather than admitting a hole if a Signal goes missing—
`gap` and the per-node `cursor` in the response say so explicitly. There are no
subscriptions, webhooks, alerts, or user-defined Signal kinds. See the [Health
Plane contract](internal/health-plane-contract.md) for exact retention,
capacity, retry, ordering, and privacy rules.

## Related references

- [Generated CLI reference](cli-reference.md)
- [CLI and HTTP parity](cli-http-parity.md)
- [Direct transport and enrollment contract](internal/direct-transport-contract.md)
- [Remote Cue contract](internal/remote-cue-contract.md)
- [Health Plane contract](internal/health-plane-contract.md)
- [Baseline delivery contract](internal/baseline-delivery.md)
