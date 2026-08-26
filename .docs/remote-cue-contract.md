# Remote Cue Contract

**Status: FROZEN CONTRACT, PENDING OWNER REVIEW.** This document freezes the wire
format, authorization mapping, idempotency rule, and every quantitative bound for
authorized remote execution. It is the go/no-go gate for plan
`remote-cue-authorized-execution` (#194). No inbound handling, dispatch,
execution, CLI, or HTTP surface may be implemented until this document is
owner-reviewed and the executable vectors in `tests/remote_cue_contract.rs` are
green.

Every number here is normative and final. There is no `TBD` and no "unspecified"
bound. A later task needing a limit not written here must amend this contract
first.

This is the first time a node accepts an instruction from outside itself. The
document is written so that a reader can answer one question at every line: *what
would an attacker have to already possess for this to matter?*

## Decision

A Cue adds **no new transport, no new signature algorithm, no new key material,
and no new capability**. It is two application message kinds carried inside the
already-frozen direct envelope, inside the already-frozen Noise session,
authorized by the already-frozen trust registry.

Three decisions carry most of the safety, and each removes a class of attack
rather than mitigating it:

1. **A Cue names a script; it never carries one.** Script content never crosses
   the wire in any form. Remote management therefore cannot introduce code onto
   a node. The worst a fully compromised Conductor can do is run something the
   node's owner already put in the workspace.
2. **Every authorization input is read from the receiver's own registry.** No
   field of an inbound message contributes to the decision to accept it. A Cue
   asserting its own role or capability is rejected when the local registry
   disagrees.
3. **A Cue-origin run is deny-all for secrets, and a script that wants a secret
   is refused at the gate rather than executed without one.**

### Sibling plane, not a sixth Health kind

`HealthKind` is closed at five (`src/health_plane/model.rs:91`) and stays closed.
Cues use a `cue_` kind namespace with a sibling signer in `direct_transport`,
reusing the private kind-agnostic `sign_envelope`
(`src/direct_transport.rs:1263`). `sign_health_envelope` (`:1150`) keeps refusing
any kind without the `health_` prefix, so it never becomes a generic signing
oracle for a plane it does not govern.

The inner frame is unchanged: `ENVELOPE_KIND = 1`
(`src/direct_transport.rs:43`). No transport code changes.

## Version and Domains

| Item | Value |
|---|---|
| `CUE_VERSION` | `1` |
| Kind prefix | `cue_` |
| Max kind bytes | `32`, the existing `MAX_ENVELOPE_KIND_BYTES` |
| Signature | BIP-340/secp256k1 over the direct envelope, unchanged |
| Envelope domain | `omakure/direct-envelope/v1\0`, unchanged |
| Run-id derivation domain | `omakure/cue-run-id/v1\0` |

The run-id derivation gets its own domain separator so a `cue_id` can never be
replayed as a signature preimage in any other construction.

## Message Kinds

Exactly two. There is no third, and adding one requires amending this document.

| Kind | Direction | Purpose |
|---|---|---|
| `cue_dispatch` | Conductor → Performer | Name one script to run, once. |
| `cue_ack` | Performer → Conductor | State the gate decision: accepted, or rejected with a stable code. |

**There is deliberately no `cue_outcome`.** The terminal result travels on the
existing `run-completed` Signal, which already carries the schema name, opaque
run id, finish time, terminal state, and exit code, and is already bounded,
durable, and idempotent. The Conductor correlates by computing the expected
opaque run id itself from the `cue_id` it sent. This is why gate D exists: a peer
without `notifications` could never deliver an outcome, so accepting its Cue
would create work whose result is unobservable.

No Signal field is added and the closed three-kind Signal set stays closed.

## `cue_dispatch` Payload

| Field | Type | Bound |
|---|---|---|
| `version` | integer | Exactly `1` |
| `cue_id` | string | 32 lowercase hex chars, the existing `OPAQUE_ID_HEX_CHARS` |
| `script` | string | `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`, at most `MAX_SCRIPT_BYTES` = 64 |
| `not_before` | integer | Unix seconds, 1..=9007199254740991 |
| `expires_at` | integer | Unix seconds, `> not_before`, `- not_before <= 300` |
| `reason` | string | 1..=128 bytes, recorded in the audit, never passed to the script |

Canonical bytes are RFC-8785, as the direct envelope already requires. Maximum
canonical `cue_dispatch` size is **512 bytes**; maximum `cue_ack` is **384**.
Both are below the existing per-kind Health caps, because a Cue carries strictly
less than a Profile.

There is no argument list, no environment map, no working directory, no timeout
override, and no payload of any kind. A Cue selects; it does not configure. Every
one of those would be an input the receiver's owner did not write.

## Authorization Mapping

Five gates, all fail-closed, all evaluated against the receiver's own registry
and configuration only. A Cue is accepted if and only if **all five** pass.

| Gate | Condition | Failure code |
|---|---|---|
| **A** | `trust.allow_remote_cues` is `true` in the receiver's own config | `1201` |
| **B** | The sender is a peer with role `conductor` and `state = 'active'` | `1202` |
| **C** | That peer holds the `remote-run` capability | `1203` |
| **D** | That peer also holds `notifications` | `1204` |
| **E** | The named script is listed in `trust.remote_cue_scripts` | `1212`, reported as `1206` |

Role is the shipped `INTEGER` encoding, `ROLE_CONDUCTOR = 1` /
`ROLE_PERFORMER = 2` (`src/health_plane/bounds.rs:13-15`). It is **not** the
`TEXT conductor/performer` sketched at `rebuild-omakure.md:510`; this contract
freezes what ships, not what was drawn.

Gate A is today **inert**: `allow_remote_cues` is parsed, defaulted to `false`,
and reported, but enforced nowhere, because nothing has ever consumed it. This
contract makes it load-bearing. A node that has never opted in refuses every Cue
regardless of how trusted the sender is.

`remote-run` requires **no capability-list amendment**: it already ships in all
three hand-duplicated copies (`src/health_plane/bounds.rs:23`,
`src/node_registry.rs:70`, `src/enrollment.rs:47`).

### Gate E: what may run is declared, not inferred

`trust.remote_cue_scripts` lists the scripts this node will run on another
node's orders. Empty or absent means **nothing**, even with
`allow_remote_cues = true`: two independent switches, both of which must be set
deliberately.

```toml
[trust]
allow_remote_cues = true
remote_cue_scripts = ["deploy.sh", "restart.lua"]
```

An earlier draft of this contract treated the `.omakureignore`-honouring
workspace listing as the allow-list. It is not one. It is a deny-list over an
implicit allow-all, and its failure mode is silent and privilege-granting: a new
file in the workspace would become remotely executable with nobody having
declared it. Privilege would be granted by forgetting rather than by acting.

Both mechanisms now apply, and they cannot conflict dangerously: a script must
be **both** discoverable and declared, so `.omakureignore` can only ever
subtract. Disagreement fails closed.

Gate E is evaluated **after** the four trust gates, so an unauthorized peer
cannot use rejection codes to learn what a node declares. And a refusal is
*audited* as `1212` but *reported* as `1206`, identically to an unresolvable
script: telling an authorized Conductor the difference between "exists but is
not declared" and "does not exist" would let it enumerate the workspace by
elimination.

## The Secret Rule

A Cue-origin run row is written with an explicit **deny-all** secret policy:
`allowed_secret_refs: Some(vec![])`. Empty already means deny-all
(`src/runs.rs:897-907`).

`None` must never be used for a Cue-origin run. `None` writes
`ALLOW_ALL_SECRET_REFS_POLICY` (`src/runs.rs:819-825`), and
`src/run_executor.rs:399` returns `SecretAccess::allow_all()` both when the
policy row is missing **and when the lookup errors**. A Cue "carrying no secrets"
written the obvious way would therefore receive *every* secret the node holds,
and a transient database error would do the same. This is the single most
dangerous default on the path and it is why the rule is a normative bound with a
vector rather than a comment.

A script whose schema declares any secret-bearing field is **rejected at the
gate** with `1205` and never executed. A remote caller does not get to decide
that a secret-consuming script should run without its secrets.

## Script Resolution

Resolution goes through the workspace repository listing, which already honours
`.omakureignore` (`src/adapters/workspace_repository.rs:168`). That listing **is**
the allow-list: a script the owner excluded from discovery is not remotely
runnable, with no second mechanism to keep in sync.

Resolution is never a string comparison on the name. The resolved path must be a
regular file by `symlink_metadata`, following the rejection pattern already used
at `src/node.rs:1057`, so a symlink cannot redirect a Cue outside the workspace.

The resolved file's content hash is recorded at accept and **re-verified at
exec**. Without this, a Cue accepted against one file could execute a different
one swapped in between the two transitions.

Failure to resolve is `1206`. The reply never distinguishes "no such script" from
"excluded from discovery": both are `1206` with identical timing-insensitive
handling, so a Cue cannot be used to enumerate a workspace.

## Idempotency and Correlation

The local run id is a deterministic function of the `cue_id` under the
run-id derivation domain. `runs.run_id` is a `TEXT PRIMARY KEY`
(`src/runs.rs:493`), so the database is the durable at-most-once key: a duplicate
`cue_dispatch` collides on insert and is answered from the existing row rather
than starting a second run.

The Conductor computes the expected `opaque_run_id`
(`src/health_plane/report.rs:517`) from the `cue_id` it sent, so correlation
needs no new field on any message.

## The At-Most-Once Rule

A Cue-origin run left `running` by a crash is **never re-claimed and never
re-executed**.

`claim_next` (`src/runs.rs:965`) currently re-claims any `running` row whose
lease expired after `HEARTBEAT_MS = 60_000`. That is correct for a queued job and
wrong for a Cue: it silently converts at-most-once into at-least-once, and the
remote caller has no way to know a side effect happened twice. `RunTrigger::Cue`
is the discriminator that excludes such rows from the lease-steal path.

A Cue-origin run whose lease expires is terminal with state `failed` and is
reported as such. Re-running it requires a new `cue_id`.

## Liveness and Revocation

- Gates are re-evaluated in the **same transaction** as the accept→run
  transition. Checking at receive and acting later is a TOCTOU window against
  revocation.
- Revocation or suspension of the sending peer **mid-run** cancels the run row.
  Trust withdrawn must stop work already in flight, or revocation is advisory.
- A revoked-then-recovered peer's pre-revocation `cue_id`s stay **permanently
  rejected**. The `revocations` table is append-only, so this is a lookup, not a
  retention policy.
- `expires_at` is checked at **both** transitions. A post-expiry replay lands
  `expired` (`1207`), never `accepted`.

## Bounds

| Bound | Value | Why this number |
|---|---|---|
| Concurrent Cue-origin runs per peer | `1` | A Conductor cannot use Cues to saturate a Performer's workers. |
| Cue dispatches per peer per minute | `10` | Matches `MAX_SIGNALS_PER_PEER_PER_MINUTE`; a Cue is rarer than a Pulse. |
| Burst allowance | `5` | The existing `RATE_BURST_ALLOWANCE`. |
| Retained Cue records per peer | `64` | Matches `SIGNAL_INBOX_CAPACITY`. |
| Cue record retention | `604800` seconds | Seven days, matching Signal retention. |
| Max lifetime (`expires_at - not_before`) | `300` seconds | A Cue is an instruction, not a schedule. |

## Error Codes

Band `1201..` inside the existing `transport_audit.error_code` range
`1000..=1999`, disjoint from transport `1001..=1011`/`1020` and Health
`1101..=1115`.

| Code | Meaning |
|---|---|
| `1201` | Remote Cues are disabled on this node (gate A) |
| `1202` | Sender is not an active peer in the conductor role (gate B) |
| `1203` | Sender lacks `remote-run` (gate C) |
| `1204` | Sender lacks `notifications` (gate D) |
| `1205` | Script declares secret-bearing fields |
| `1206` | Script not resolvable in the discoverable workspace |
| `1207` | Cue expired or not yet valid |
| `1208` | Duplicate `cue_id`; answered from the existing run |
| `1209` | Cue rate bound exceeded |
| `1210` | A Cue-origin run for this peer is already in flight |
| `1211` | Malformed payload, size, or grammar violation |
| `1212` | Script is not declared in `trust.remote_cue_scripts` (reported as `1206`) |

Every rejection writes a durable redacted audit row. `reason` is recorded;
nothing derived from the sender is ever interpolated into a command.

## Audit

Each accept and each rejection writes one `transport_audit` row carrying the
sender node id, the `cue_id`, the outcome, and the code. The script name is
recorded; the `reason` string is recorded verbatim as data. No payload field is
ever logged in a position where it could be read back as a command.

## Explicitly Out of Scope

Requiring a further owner-approved amendment:

- A Cue that carries script content, arguments, environment, or any execution
  parameter.
- Any HTTP surface for dispatch. `node cue` is CLI-only; `cli-http-parity.md`
  has a `CLI-only` status for recording that honestly.
- A Conductor-side durable outbox. Dispatch is a one-shot dial mirroring the
  existing `probe()`.
- A separate multi-state `inbox` table. `RunState` already models this, and a
  second state machine would need a `node.sqlite` bump that is a compile error
  until the frozen bound, the fixture, and this contract move together.
- Baselines, MDM, fan-out to more than one peer per dispatch, and scheduling.
