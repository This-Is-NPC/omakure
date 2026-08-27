# Health Plane Contract

**Status: FROZEN CONTRACT, PENDING OWNER REVIEW.** This document freezes the
minimal Health Plane wire format, authorization mapping, state model, and every
quantitative bound for Profile, Pulse, and the closed Signal lifecycle. It is the
go/no-go gate for plan `health-plane-foundation` (#190). No Health Plane
persistence, production message handling, CLI surface, or HTTP projection may be
implemented until this document is owner-reviewed and the executable vectors in
`tests/health_plane_contract.rs` are green.

Every number in this document is normative and final. There is no `TBD` and no
"unspecified" bound. A later task that needs a limit not written here must amend
this contract first.

## Decision

The Health Plane adds **no new transport, no new signature construction, no new
key material, and no new capability**. It is a closed set of five application
message kinds carried inside the already-frozen direct envelope, inside the
already-frozen Noise transport, authorized by the already-frozen trust registry.

The frozen direct-transport identity construction is unchanged and MUST NOT be
altered by any Health Plane work:

- BIP-340/secp256k1 remains the sole node and application signature algorithm.
- The static X25519 transport key remains transport-only and never signs a
  Health Plane message.
- `Noise_XX_25519_ChaChaPoly_SHA256`, the prologue, the 245-byte transport
  certificate, the outer frame, and the encrypted inner framing are unchanged.
- The direct envelope signature domain `omakure/direct-envelope/v1\0` and its
  RFC-8785 canonical JSON prehash are reused verbatim.

See `.docs/direct-transport-contract.md` for those frozen constructions. This
document only adds envelope `kind` values, payload schemas, authorization rules,
and bounds.

## Version and Domains

| Name | Exact value |
|---|---|
| Contract ID | ASCII `omakure/health-plane/v1` |
| Health Plane message-schema version | integer `1`, carried as `payload.health_version` |
| Direct envelope version | integer `1`, carried as envelope `version` (frozen, unchanged) |
| Signature domain | Existing `omakure/direct-envelope/v1` followed by one NUL byte (frozen, unchanged) |
| Node ID domain | Existing `omakure/node-id/v1` followed by one NUL byte (frozen, unchanged) |
| Node registry schema version | `8` (version 7 created the Health Plane tables; version 8 widened the stored Profile with the two baseline fields) |

`payload.health_version` is deliberately independent of the envelope `version`.
The envelope version governs the signed container; the Health Plane version
governs the payload schema. A future Health Plane version increments
`payload.health_version` and MUST NOT be selected by removing fields from
version 1, by omitting `health_version`, or by silent fallback.

## Carriage

A Health Plane message is exactly one direct envelope:

1. It is signed with the frozen construction:
   `BIP-340 over SHA-256(ASCII("omakure/direct-envelope/v1"), NUL, canonical)`
   where `canonical` is the strict RFC-8785 canonical UTF-8 serialization of the
   envelope object without its signature.
2. Its encoded form is `canonical || signature:64`.
3. It is written as encrypted transport inner kind `1` (direct envelope) on an
   established Noise session, with the frozen inner plaintext layout
   `sequence:u64be || inner_kind:u8(1) || inner_version:u8(1) || inner_body`.
4. It uses no new outer frame kind, no new inner kind, and no new control frame.

The envelope object has exactly the seven frozen top-level fields, in RFC-8785
key order: `created_at`, `kind`, `nonce`, `payload`, `sender`, `session_id`,
`version`. Any additional or missing top-level field is rejected.

| Envelope field | Rule for Health Plane messages |
|---|---|
| `created_at` | Unix seconds, integer, `1` .. `9007199254740991`; freshness rules below |
| `kind` | Exactly one of the five kinds in the next table |
| `nonce` | 16 random bytes as 32 lowercase hex characters, CSPRNG, unique per session |
| `payload` | The Health Plane payload object defined below |
| `sender` | The canonical 69-byte `omk1_` node ID of the signing node |
| `session_id` | The 32-byte Noise final handshake hash of the carrying session, 64 lowercase hex characters |
| `version` | Integer `1` |

Because `payload` is inside the canonical bytes, the target binding, message ID,
sequence, and every health fact are covered by the BIP-340 signature. No Health
Plane field is carried outside the signature.

## Message Kinds

The set is closed. There are exactly five kinds and there is no generic event
bus, subscription, webhook, or arbitrary payload kind.

| `kind` | Direction | Purpose |
|---|---|---|
| `health_profile` | Performer to Conductor | Latest static node facts |
| `health_pulse` | Performer to Conductor | Latest liveness and runner health |
| `health_signal` | Performer to Conductor | One closed-lifecycle event |
| `health_ack` | Conductor to Performer | Positive acknowledgement plus cursor |
| `health_error` | Conductor to Performer | Stable rejection code |

A `health_profile`, `health_pulse`, or `health_signal` received by a node acting
as a Performer for that peer is rejected with `health_wrong_role` (1105). A
`health_ack` or `health_error` received from a peer that is not the local
Conductor is rejected with `health_wrong_role` (1105).

## Common Payload Header

Every Health Plane payload begins with exactly these three fields, and each kind
adds exactly one body field.

| Field | Type | Rule |
|---|---|---|
| `health_version` | integer | Exactly `1` |
| `message_id` | string | 16 bytes as 32 lowercase hex characters, CSPRNG, unique per sender |
| `target` | string | Canonical 69-byte `omk1_` node ID of the intended receiver |

`target` MUST equal the receiver's own active node ID. Any other value, including
a syntactically valid node ID of a third party, is rejected with
`health_wrong_target` (1104) before any state is read or written.

`message_id` is the idempotency and replay key. It is recorded before any health
state is applied.

## Profile

`kind = "health_profile"`, body field `profile`. Profile carries only static node
facts. It is latest-state only: a Conductor retains exactly one Profile row per
Performer and replaces it in place.

```json
{
  "health_version": 1,
  "message_id": "00000000000000000000000000000001",
  "target": "omk1_<64 lowercase hex>",
  "profile": {
    "agent_version": "0.3.0",
    "arch": "x86_64",
    "baseline_id": "3f0a91c4d2b85e67a1c30f4e8b29d75641aeb0c3928f5d61b7e04a2c8d9f1350",
    "baseline_observed_id": "3f0a91c4d2b85e67a1c30f4e8b29d75641aeb0c3928f5d61b7e04a2c8d9f1350",
    "capabilities": ["inventory-health", "notifications"],
    "display_name": "workshop-laptop",
    "distro_id": "arch",
    "distro_version": "rolling",
    "omarchy_channel": "stable",
    "omarchy_version": "2.1.0",
    "platform": "linux",
    "profile_revision": 1,
    "role": "performer",
    "runtimes": [
      {"available": true, "name": "bash", "version": "5.2.37"},
      {"available": false, "name": "powershell", "version": ""}
    ]
  }
}
```

| Field | Type | Frozen rule |
|---|---|---|
| `agent_version` | string | 1..=32 bytes, `[0-9][0-9A-Za-z.+-]{0,31}` |
| `arch` | string | Exactly one of `x86_64`, `aarch64`, `unknown` |
| `baseline_id` | string | `` (empty) or exactly 64 lowercase hex characters |
| `baseline_observed_id` | string | `` (empty) or exactly 64 lowercase hex characters; MUST be empty when `baseline_id` is empty |
| `capabilities` | array of string | 0..=32 entries, each 1..=64 bytes, each from the frozen allow-list, sorted by raw bytes, unique |
| `display_name` | string | 0..=64 bytes, `` (empty) or `[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}` with no trailing space |
| `distro_id` | string | 0..=32 bytes, `` (empty) or `[a-z0-9][a-z0-9._-]{0,31}` |
| `distro_version` | string | 0..=32 bytes, `` (empty) or `[0-9A-Za-z][0-9A-Za-z._+-]{0,31}` |
| `omarchy_channel` | string | Exactly one of `` (empty), `stable`, `dev` |
| `omarchy_version` | string | 0..=32 bytes, `` (empty) or `[0-9A-Za-z][0-9A-Za-z._+-]{0,31}` |
| `platform` | string | Exactly one of `linux`, `macos`, `windows` |
| `profile_revision` | integer | 1..=9007199254740991, strictly increasing per sender |
| `role` | string | Exactly `performer` |
| `runtimes` | array of object | 0..=4 entries, sorted by `name`, unique by `name` |
| `runtimes[].available` | boolean | Required |
| `runtimes[].name` | string | Exactly one of `bash`, `powershell`, `python`, `sh` |
| `runtimes[].version` | string | 0..=32 bytes, `` (empty) or `[0-9A-Za-z][0-9A-Za-z._+-]{0,31}`; MUST be empty when `available` is `false` |

The frozen capability allow-list is unchanged from the transport contract and
from `SUPPORTED_CAPABILITIES` in `src/node_registry.rs` and `src/enrollment.rs`:
`backup-orchestration`, `baseline-push`, `inventory-health`,
`lost-device-revocation`, `notifications`, `remote-run`,
`ssh-credential-rotation`.

`capabilities` reports the capabilities the Performer has been granted by this
Conductor. It is an echo for operator visibility; the receiver authorizes from
its own registry and never from this field.

### The baseline pair

`baseline_id` and `baseline_observed_id` are the amendment item 8 needs, and
they are two fields rather than one on purpose.

`baseline_id` is the derived name of the set this Performer recorded
installing — the **claim**. `baseline_observed_id` is the same derivation
(`.docs/baseline-delivery.md`, domain `omakure/baseline-id/v1\0`) recomputed
over the paths that set named, as they are on this node's disk now — the
**evidence**. Both are 32 bytes wide because that is what the baseline plane
derives; the width is not a policy choice this contract made, and
`src/health_plane/bounds.rs` derives it from `crate::baseline` rather than
transcribing it, so the two cannot disagree.

**A Performer never reports drift.** It does not know what it was supposed to
have; it knows what it recorded installing and what it can currently see. The
comparison is the Conductor's, and it is a comparison of two facts rather than a
verdict taken on trust:

| `baseline_id` | `baseline_observed_id` | Fleet projection |
|---|---|---|
| No Profile stored | — | `unknown` — this Performer has not reported yet |
| empty | empty | `none` — this node holds no baseline |
| set | equal to it | `in_sync` |
| set | different | `drifted` |

Empty is the only way to say "no baseline", and it can never collide with an
identity: an empty entry list is not signable (`src/baseline.rs` refuses it), so
no baseline that was ever pushed can name itself with the empty string.
Evidence without a claim is refused with `health_invalid_message` (1102) — a
node that recorded nothing cannot have observed something, and accepting the
pair would store a verdict no set on disk could justify.

What this pair does **not** cover, stated rather than implied: the identity
names the set that was published, so a file added to the workspace that no
baseline entry names does not change it. Drift here means "the set that was
installed is no longer what was installed", not "nothing else exists on this
machine".

No other Profile field exists in version 1. Hostname, username, IP address, MAC
address, serial number, disk encryption state, installed packages, process list,
filesystem paths, and resource gauges are forbidden (see Privacy Classes).

## Pulse

`kind = "health_pulse"`, body field `pulse`. Pulse carries liveness plus runner
and last-run health. It is latest-state only: a Conductor retains exactly one
Pulse row per Performer and replaces it in place.

```json
{
  "health_version": 1,
  "message_id": "00000000000000000000000000000002",
  "target": "omk1_<64 lowercase hex>",
  "pulse": {
    "emitted_at": 1700000000,
    "last_run": {
      "exit_code": 0,
      "finished_at": 1699999990,
      "run_id": "0000000000000000000000000000000a",
      "script": "deploy",
      "started_at": 1699999980,
      "state": "completed",
      "trigger": "scheduled"
    },
    "profile_revision": 1,
    "runner": {
      "queue_depth": 0,
      "scheduler": "running",
      "state": "idle",
      "workers_busy": 0,
      "workers_configured": 1
    },
    "sequence": 1,
    "uptime_seconds": 3600
  }
}
```

| Field | Type | Frozen rule |
|---|---|---|
| `emitted_at` | integer | Unix seconds, 1..=9007199254740991, MUST equal the envelope `created_at` |
| `last_run` | object or null | `null` when the node has never completed a run |
| `last_run.exit_code` | integer or null | -256..=255, or `null` when the run produced no exit code |
| `last_run.finished_at` | integer | Unix seconds, >= `started_at` |
| `last_run.run_id` | string | 16 bytes as 32 lowercase hex characters, opaque |
| `last_run.script` | string | 1..=64 bytes, `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`; the schema `Name` only |
| `last_run.started_at` | integer | Unix seconds, 1..=9007199254740991 |
| `last_run.state` | string | Exactly one of `completed`, `failed`, `cancelled`, `timed_out`, `dead_letter` |
| `last_run.trigger` | string | Exactly one of `manual`, `scheduled`, `queue`, `cue` |
| `profile_revision` | integer | 0..=9007199254740991; the `profile_revision` this Pulse corresponds to, `0` before any Profile was sent |
| `runner.queue_depth` | integer | 0..=65535 |
| `runner.scheduler` | string | Exactly one of `running`, `disabled` |
| `runner.state` | string | Exactly one of `idle`, `busy`, `paused`, `degraded`, `stopped` |
| `runner.workers_busy` | integer | 0..=255, `<= workers_configured` |
| `runner.workers_configured` | integer | 0..=255 |
| `sequence` | integer | 1..=9007199254740991, strictly increasing per (sender, target) |
| `uptime_seconds` | integer | 0..=4294967295 |

Version 1 has exactly five numeric runner/liveness values: `queue_depth`,
`workers_busy`, `workers_configured`, `uptime_seconds`, and `sequence`. **No CPU,
memory, disk, network, temperature, battery, load-average, or any other gauge is
permitted.** Adding one is an arbitrary metric and is excluded from this plan.

Pulse never carries run arguments, run output, trace bodies, or a run's script
path. `run_id` is opaque and `script` is a schema name.

## Signal

`kind = "health_signal"`, body field `signal`. The lifecycle is closed: there are
exactly three Signal kinds and no mechanism to add a fourth on the wire.

```json
{
  "health_version": 1,
  "message_id": "00000000000000000000000000000003",
  "target": "omk1_<64 lowercase hex>",
  "signal": {
    "kind": "run-completed",
    "occurred_at": 1699999990,
    "run": {
      "exit_code": 0,
      "finished_at": 1699999990,
      "run_id": "0000000000000000000000000000000a",
      "script": "deploy",
      "state": "completed"
    },
    "sequence": 1,
    "signal_id": "0000000000000000000000000000000b",
    "subject": null
  }
}
```

All six `signal` fields are always present. The one that does not apply to this
`kind` is explicitly `null`; it is never omitted, because an omitted field is
`health_unknown_field` (1114).

| Field | Type | Frozen rule |
|---|---|---|
| `kind` | string | Exactly one of `enrolled`, `revoked`, `run-completed` |
| `occurred_at` | integer | Unix seconds, 1..=9007199254740991, `<=` envelope `created_at` |
| `run` | object or null | Present exactly when `kind == "run-completed"`, otherwise `null` |
| `run.exit_code` | integer or null | -256..=255, or `null` |
| `run.finished_at` | integer | Unix seconds, MUST equal `occurred_at` |
| `run.run_id` | string | 16 bytes as 32 lowercase hex characters |
| `run.script` | string | 1..=64 bytes, `[A-Za-z0-9][A-Za-z0-9._-]{0,63}` |
| `run.state` | string | Exactly one of `completed`, `failed`, `cancelled`, `timed_out`, `dead_letter` |
| `sequence` | integer | 1..=9007199254740991, strictly increasing per (sender, target) |
| `signal_id` | string | 16 bytes as 32 lowercase hex characters, stable idempotency key |
| `subject` | string or null | Present exactly when `kind` is `enrolled` or `revoked`; the canonical 69-byte `omk1_` node ID of the affected peer, otherwise `null` |

Exactly one of `run` and `subject` is non-null. Both non-null, both null, or the
wrong one for the `kind` is rejected with `health_invalid_message` (1102).

`signal_id` is the application idempotency key: applying the same `signal_id`
twice MUST NOT produce a second stored Signal. `message_id` is the transport-level
replay key. A resend of the same logical Signal reuses `signal_id` and
`sequence`, but MUST use a fresh `message_id` and a fresh envelope `nonce`.

Signals are a small bounded inbox/outbox, not history. See Storage Bounds.

## Acknowledgement and Error

`kind = "health_ack"`, body field `ack`:

```json
{
  "health_version": 1,
  "message_id": "00000000000000000000000000000004",
  "target": "omk1_<64 lowercase hex>",
  "ack": {
    "accepted": true,
    "acked_message_id": "00000000000000000000000000000003",
    "cursor": 1
  }
}
```

`kind = "health_error"`, body field `error`:

```json
{
  "health_version": 1,
  "message_id": "00000000000000000000000000000005",
  "target": "omk1_<64 lowercase hex>",
  "error": {
    "accepted": false,
    "acked_message_id": "00000000000000000000000000000003",
    "code": 1110,
    "reason": "health_replay"
  }
}
```

| Field | Type | Frozen rule |
|---|---|---|
| `ack.accepted` | boolean | Always `true` |
| `ack.acked_message_id` | string | 32 lowercase hex characters; the `message_id` being acknowledged |
| `ack.cursor` | integer | 0..=9007199254740991; the receiver's highest contiguously accepted Signal `sequence` for this sender |
| `error.accepted` | boolean | Always `false` |
| `error.acked_message_id` | string | 32 lowercase hex characters; the `message_id` being rejected |
| `error.code` | integer | One of the frozen codes 1101..=1115 |
| `error.reason` | string | The stable snake_case name of `error.code`, 1..=32 bytes |

An error carries only the stable code and its name. It never carries the
offending bytes, a field value, a signature, a key, a path, or a diagnostic
string. A `health_error` is emitted only after the sender is authenticated,
authorized, and target-bound; before that point the receiver drops the message,
audits it, and sends nothing.

## Error Codes

Health Plane codes occupy `1101..=1115`, disjoint from the frozen transport codes
`1001..=1011` and inside the existing `transport_audit.error_code` range
`1000..=1999`.

| Code | Name | Meaning |
|---:|---|---|
| 1101 | `health_unsupported_version` | `health_version` is not `1` |
| 1102 | `health_invalid_message` | Encoding, canonicalization, type, range, grammar, or field-combination failure |
| 1103 | `health_message_too_large` | Canonical bytes exceed the per-kind cap |
| 1104 | `health_wrong_target` | `target` is not the receiver's active node ID |
| 1105 | `health_wrong_role` | Peer role does not permit this kind in this direction |
| 1106 | `health_missing_capability` | Peer lacks the required capability |
| 1107 | `health_revoked` | Peer identity or trust is revoked |
| 1108 | `health_stale` | `created_at` is older than the freshness window |
| 1109 | `health_future` | `created_at` is beyond the accepted future skew |
| 1110 | `health_replay` | Duplicate `message_id`, `signal_id`, `sequence`, or `profile_revision` |
| 1111 | `health_reordered` | Signal `sequence` is beyond the reorder window |
| 1112 | `health_rate_limited` | A Health Plane rate or replay-capacity limit was reached |
| 1113 | `health_queue_full` | Inbox or outbox capacity was reached |
| 1114 | `health_unknown_field` | An unknown or missing field name at any depth |
| 1115 | `health_corrupt_state` | Local Health Plane state failed integrity checks |

Every rejection is bounded, allocation-safe, mutation-free for identity, trust,
revocation, and run history, and audited with the stable code.

## Receive Order

A receiver evaluates a Health Plane message in exactly this order. Each step
fails closed and stops.

1. **Transport.** The frozen transport path already completed: outer frame,
   session ID, Noise decryption, inner sequence, and envelope BIP-340 signature.
   The session's authenticated node ID is the sender identity. A Health Plane
   message on a session in state `authenticated_untrusted` is dropped without a
   reply (the frozen transport already forbids application envelopes there).
2. **Size.** Reject with `health_message_too_large` (1103) if the canonical bytes
   exceed the per-kind cap, before parsing.
3. **Envelope shape.** Exactly seven top-level fields, canonical re-encoding
   equality, `version == 1`, `sender` equal to the session's authenticated node
   ID. Otherwise `health_invalid_message` (1102).
4. **Version.** `payload.health_version == 1`, else `health_unsupported_version`
   (1101).
5. **Schema.** Strict closed-schema validation of every field name, type, range,
   grammar, ordering, and combination. Any unknown or missing field name gives
   `health_unknown_field` (1114); any other schema failure gives
   `health_invalid_message` (1102).
6. **Target.** `payload.target` equals the receiver's active node ID, else
   `health_wrong_target` (1104).
7. **Trust.** The sender is an `active` row in `remote_identities` and an
   `active` row in `trusted_peers`, else `health_revoked` (1107).
8. **Role.** The peer role permits this kind in this direction, else
   `health_wrong_role` (1105).
9. **Capability.** The required capability is present in the peer's granted set,
   else `health_missing_capability` (1106).
10. **Freshness.** `created_at` is inside the freshness window, else
    `health_stale` (1108) or `health_future` (1109).
11. **Rate.** The peer is inside every Health Plane rate limit, else
    `health_rate_limited` (1112).
12. **Replay.** `message_id` is not already recorded, else `health_replay`
    (1110). The `message_id` is recorded before any health state is applied.
13. **Ordering.** Sequence, revision, and cursor rules below, else
    `health_replay` (1110) or `health_reordered` (1111).
14. **Capacity.** Inbox or outbox space is available, else `health_queue_full`
    (1113).
15. **Apply.** Health state is written in one transaction, then `health_ack` is
    sent.

Steps 1 through 6 never touch persistent state. Step 7 onward may read the trust
registry but MUST NOT mutate identity, trust, capability, revocation, transport
session, or run state under any outcome. The Health Plane can only ever write
Health Plane rows and Health Plane audit rows.

### Transport-layer failure mapping

Step 1 uses the frozen transport verification path
(`omakure::direct_transport::verify_envelope`). Its `TransportError` values map
to stable Health Plane codes exactly as follows, so a transport-level failure
never produces an unspecified Health Plane outcome:

| `TransportError` | Health Plane code | Cause |
|---|---:|---|
| `HandshakeFailed` | 1102 `health_invalid_message` | BIP-340 signature verification failed |
| `IdentityMismatch` | 1102 `health_invalid_message` | `version`, `sender`, or `kind` disagreed with the session |
| `Replay` | 1110 `health_replay` | `session_id` or `nonce` did not match this session |
| `InvalidFrame` | 1102 `health_invalid_message` | Encoding or canonicalization failure |
| `MessageTooLarge` | 1103 `health_message_too_large` | Size limit exceeded |
| Any other variant | 1102 `health_invalid_message` | Bounded rejection |

A failure at step 1 is dropped and audited without a `health_error` reply,
because the sender is not yet proven authorized and target-bound.

## Authorization Mapping

**No new capability is introduced.** The two existing capabilities in the frozen
allow-list are sufficient and the feasibility probe proves it.

| Kind | Required peer role on the receiver | Required capability |
|---|---|---|
| `health_profile` | `2` (`performer`) | `inventory-health` |
| `health_pulse` | `2` (`performer`) | `inventory-health` |
| `health_signal` | `2` (`performer`) | `notifications` |
| `health_ack` | `1` (`conductor`) | none |
| `health_error` | `1` (`conductor`) | none |

Rationale for zero new capabilities: Profile and Pulse are exactly the
"inventory and health" surface the `inventory-health` capability names, and the
closed three-kind Signal feed is exactly the "notifications" surface. A Performer
that grants `inventory-health` but not `notifications` reports Profile and Pulse
and refuses Signals, which is a useful and enforceable posture. Acknowledgements
carry no health data and therefore require role only.

Role is read from `trusted_peers.role` (`1 = conductor`, `2 = performer`) and
capability from `trusted_peers.capabilities` on the receiving node. Both columns
already exist in the shipped schema, are written by the production `node trust`
path, are updated by `node capabilities`, and are set to `state = 'revoked'` by
`node revoke`. The Health Plane never authorizes from a field inside the message,
from the node ID, from the certificate, from the discovery beacon, or from a
successful handshake.

**Required read-only projection.** The shipped
`NodeRegistry::transport_peer(node_id, public_key_hex) -> Option<TransportPeer>`
deliberately returns only `node_id`, `identity_key`, `transport_public_key`,
`key_epoch`, and `state`; it carries no role and no capability set, and
`NodeRegistry::peer(node_id)` reads the legacy version-1 `peers` table that the
runtime transport path intentionally does not consult. Health Plane
authorization therefore requires exactly one new **read-only** registry
projection that returns `role` and `capabilities` from `trusted_peers` alongside
the existing `state`. This is a new query, not a new capability, not a new trust
state, and not a schema change to the trust tables. The feasibility probe proves
the data is present and decisive on the live `node.sqlite` of a running
production service. No other registry change is authorized by this contract.

A revoked peer, an inactive identity, or a peer with no `trusted_peers` row is
rejected at step 7 with `health_revoked` (1107) before role or capability is
examined.

## Freshness, Clock Skew, and Presence

| Rule | Frozen value |
|---|---:|
| Maximum accepted age of `created_at` | 120 seconds |
| Maximum accepted future skew of `created_at` | 60 seconds |
| Boundary behavior | Inclusive at exactly 120 seconds old and exactly 60 seconds ahead; one second beyond either bound rejects |
| Clock source | UTC Unix seconds |

The Health Plane window is deliberately tighter than the frozen 300-second
certificate skew because Health Plane data is live state, not a long-lived
credential. A node whose clock is outside the window fails closed; it never
adjusts its clock from a peer message.

Presence is a Conductor-local projection derived from the last accepted Pulse. It
is never carried on the wire.

| Presence | Frozen rule |
|---|---|
| `unknown` | No Pulse has ever been accepted from this Performer |
| `online` | Last accepted Pulse is 0..=90 seconds old |
| `stale` | Last accepted Pulse is 91..=600 seconds old |
| `offline` | Last accepted Pulse is more than 600 seconds old |

## Ordering, Duplicates, and Cursor

| Rule | Frozen value |
|---|---|
| Pulse ordering | `pulse.sequence` strictly greater than the last accepted Pulse sequence for that sender; equal or lower is `health_replay` (1110) |
| Profile ordering | `profile.profile_revision` strictly greater than the last accepted revision for that sender; equal or lower is `health_replay` (1110) |
| Pulse/Profile reordering | Not buffered; latest-state only |
| Signal cursor | Per (sender, target); starts at `0`; the first accepted Signal MUST have `sequence == 1` |
| Signal in order | `sequence == cursor + 1` is accepted and the cursor advances by exactly 1 |
| Signal duplicate | `sequence <= cursor`, or a `signal_id` already stored, is `health_replay` (1110) and MUST NOT store a second row |
| Signal gap | `cursor + 1 < sequence <= cursor + 32` is held in the reorder buffer |
| Reorder buffer size | 32 Signals per sender |
| Reorder buffer lifetime | 60 seconds per held Signal |
| Signal far-future | `sequence > cursor + 32` is `health_reordered` (1111) and is not buffered |
| Reorder timeout | A held Signal older than 60 seconds is discarded with `health_reordered` (1111); the cursor does NOT advance past the gap |
| Gap recovery | The Conductor acknowledges with its current `cursor`; the Performer resends from `cursor + 1` using fresh `message_id` values and the original `signal_id`/`sequence` |

The cursor never moves backwards and never skips. A Conductor that cannot obtain
`cursor + 1` stalls that Performer's Signal feed rather than accepting a hole.

The reference receiver in `tests/health_plane_contract.rs` models the reorder
buffer with a lifetime of zero, so a gap produces `health_reordered` (1111)
immediately. A production implementation holds the out-of-order Signal for up to
60 seconds first and produces the same stable code when the gap does not fill.
The observable outcome for the Performer is identical: the acknowledged cursor
does not advance, and the Performer resends from `cursor + 1`.

## Replay Protection

| Rule | Frozen value |
|---|---:|
| Replay key | `payload.message_id`, 16 bytes |
| Recorded | Before any health state is applied, inside the same transaction |
| Security floor (never evict younger than) | 180 seconds (120 s freshness + 60 s skew) |
| Nominal retention | 900 seconds |
| Maximum replay rows | 131,072 |
| Eviction | Oldest `expires_at` first, only rows past the 180-second floor |
| Capacity exhaustion | If the cap is reached and no row is evictable, reject with `health_rate_limited` (1112); never accept unprotected |

Retention is safe at 900 seconds because a message older than 120 seconds is
already rejected as `health_stale` regardless of replay state.

`signal_id` provides application idempotency independently of `message_id`: a
retransmitted Signal with a fresh `message_id` and an already-stored `signal_id`
is rejected with `health_replay` (1110) and never applied twice.

## Rate Bounds

| Rule | Frozen value |
|---|---:|
| Nominal Pulse interval | 30 seconds |
| Minimum accepted interval between accepted Pulses from one peer | 10 seconds |
| Maximum Health Plane messages accepted per peer per minute | 20 |
| Maximum `health_profile` accepted per peer per hour | 12 |
| Maximum `health_signal` accepted per peer per minute | 10 |
| Burst allowance above the per-minute limit | 5 messages |
| In-flight unacknowledged messages per session | 8 |
| Rate key | The authenticated peer `node_id` |
| Over-limit behavior | Reject with `health_rate_limited` (1112), no state mutation, audited |

The Health Plane rate limits are applied in addition to, never instead of, the
frozen transport limits (1,024 concurrent peers, 4 sessions per peer, 4
unauthenticated handshakes per source per minute, 64 queued frames per session).

## Node-Count Bounds

| Rule | Frozen value |
|---|---:|
| Maximum Performers tracked by one Conductor | 256 |
| Maximum Conductors per Performer | 1 |
| Behavior at the Performer cap | Reject the 257th peer's Health Plane messages with `health_queue_full` (1113); trust is unchanged |
| Behavior on a second Conductor | Reject with `health_wrong_role` (1105); a manager change requires an explicit authorized enrollment update |

256 is deliberately below the frozen transport peer limit of 1,024 so the Health
Plane can never be the component that exhausts transport capacity.

## Queue, Retry, and Timeout Bounds

| Rule | Frozen value |
|---|---:|
| Performer Signal outbox | 64 Signals |
| Outbox overflow | Drop the oldest undelivered Signal, increment a local `signals_dropped` counter, audit once per drop |
| Profile/Pulse outbox | 1 pending each; a newer Profile or Pulse replaces the pending one and never appends |
| Conductor Signal inbox per Performer | 64 Signals |
| Conductor global Signal inbox | 16,384 Signals (256 x 64) |
| Inbox overflow | Reject with `health_queue_full` (1113); the Performer retains the Signal in its outbox |
| Acknowledgement timeout | 5 seconds |
| Retries per message | 3 (matches the shipped `MAX_RETRIES`) |
| Retry backoff | 1 s, 2 s, 4 s (matches the shipped `RETRY_BACKOFF_SECONDS`) |
| After the final retry, Profile/Pulse | Dropped; the next scheduled Profile/Pulse supersedes it |
| After the final retry, Signal | Retained in the outbox within its 64-entry and 7-day bounds; resent on the next session |
| Receiver processing budget per message | 250 milliseconds |
| Budget exceeded | Abort the transaction, reject with `health_corrupt_state` (1115), audit; no partial apply |
| Session establishment reuse | The Health Plane never opens a session; it uses an existing established session or waits |

## Storage and Retention Bounds

| Rule | Frozen value |
|---|---:|
| Profile rows per Performer | 1 (latest only) |
| Pulse rows per Performer | 1 (latest only) |
| Signal rows per Performer | 64 |
| Signal retention | 604,800 seconds (7 days) |
| Signal eviction | Oldest first by `(occurred_at, signal_id)` when either the 64-row or 7-day bound is exceeded |
| Maximum stored Profile row | 2,112 bytes (the encoded message) |
| Maximum stored Pulse row | 1,344 bytes (the encoded message) |
| Maximum stored Signal row | 1,088 bytes (the encoded message) |
| Worst-case bytes per Performer | 73,088 (2,112 + 1,344 + 64 x 1,088) |
| Worst-case Health Plane payload bytes at 256 Performers | 18,710,528 |
| Maximum replay rows | 131,072 at 32 bytes = 4,194,304 bytes |
| Health Plane audit rows | 10,000 at 256 bytes = 2,560,000 bytes |
| Health Plane audit retention | 2,592,000 seconds (30 days) or the 10,000-row cap, whichever is reached first |
| **Total frozen Health Plane storage ceiling** | **25,464,832 bytes** |

The ceiling is a hard budget. An implementation that would exceed it MUST evict
within these rules or fail closed; it MUST NOT grow.

## Message Size Bounds

Sizes are of the canonical envelope bytes, excluding the 64-byte signature.

| Kind | Maximum canonical bytes | Maximum encoded bytes (canonical + signature) | Measured worst case |
|---|---:|---:|---:|
| `health_profile` | 2,048 | 2,112 | 1,504 |
| `health_pulse` | 1,280 | 1,344 | 926 |
| `health_signal` | 1,024 | 1,088 | 777 |
| `health_ack` | 768 | 832 | 510 |
| `health_error` | 768 | 832 | 541 |

The measured worst case is the canonical size of a message in which every
bounded string, array, and integer is at its frozen maximum. It is produced and
asserted by `tests/health_plane_contract.rs`, so a future field addition that
would breach a cap fails the contract test instead of failing in production.

| Structural bound | Frozen value |
|---|---:|
| Maximum JSON nesting depth of the envelope object | 5 |
| Maximum field-name length at any depth | 32 bytes |
| Maximum total field count in one payload | 64 |
| Maximum array length anywhere | 32 |
| Maximum string length anywhere | 128 bytes |

Every Health Plane message is therefore at most 2,112 encoded bytes, which is
0.2 percent of the frozen 1,048,520-byte plaintext limit. A message that exceeds
its per-kind cap is rejected with `health_message_too_large` (1103) before JSON
parsing and before any allocation proportional to the declared content.

## Unknown Version, Unknown Fields, and Mixed Versions

Version 1 uses a **strict closed schema**. There is no forward-compatible
"ignore unknown fields" behavior, because ignoring an unknown field would let an
attacker smuggle unvalidated content past a signature check that the operator
believes covers the whole message.

| Case | Frozen behavior |
|---|---|
| `health_version` missing | `health_unknown_field` (1114) |
| `health_version` not `1` | `health_unsupported_version` (1101) |
| Unknown top-level payload field | `health_unknown_field` (1114) |
| Unknown field inside `profile`, `pulse`, `signal`, `ack`, `error`, `runner`, `last_run`, `run`, or `runtimes[]` | `health_unknown_field` (1114) |
| Missing required field | `health_unknown_field` (1114) |
| Duplicate JSON key | `health_invalid_message` (1102) |
| Non-canonical key order, whitespace, number form, or escape | `health_invalid_message` (1102) |
| Floating-point or non-integer number | `health_invalid_message` (1102) |
| Non-shortest-form UTF-8, NUL, or control character in any string | `health_invalid_message` (1102) |
| Unknown envelope `kind` beginning with `health_` | `health_unknown_field` (1114) |

Mixed-version policy:

- A Conductor that receives `health_version != 1` replies `health_error` 1101,
  marks that Performer `version_incompatible` in its local projection, keeps the
  peer's trust and transport state unchanged, and continues serving every other
  peer.
- A Performer that receives `health_error` 1101 stops sending Health Plane
  messages to that Conductor for 300 seconds, then retries at most once per
  300 seconds.
- Neither side ever downgrades, omits fields, or negotiates. A future version
  uses a new `health_version` value and a new copy of this contract.
- The version-incompatible state expires after 3,600 seconds without a further
  1101, so a Conductor upgrade heals without operator action.

## Privacy Classes and Redaction

**Class P0, permitted on the wire.** The complete list; nothing else is P0.

Node ID, target node ID, session ID, nonce, message ID, signal ID, run ID,
baseline ID and observed baseline ID (derived digests over a published script
set, carrying no path and no host fact),
sequence, cursor, profile revision, Unix timestamps, role name, granted
capability names from the frozen allow-list, agent version, platform, arch,
distro id, distro version, Omarchy version, Omarchy channel, operator-chosen
display name, runtime names from the closed four-name set, runtime versions,
runtime availability, runner state, scheduler state, worker counts, queue depth,
uptime seconds, script schema name, run state, run trigger, run exit code, error
code, and error reason name.

**Class P1, forbidden anywhere in a Health Plane message, at any depth, in any
encoding.** A message containing any of these is rejected, never redacted and
never stored:

Secret values, resolved `secret://` references, bearer or API tokens, Argon2id
hashes, private keys, transport private keys, enrollment codes, raw script
arguments, script source, script stdout or stderr, trace bodies, environment
variable values, workspace paths, filesystem paths, hostnames, usernames, home
directories, IP addresses, MAC addresses, serial numbers, geolocation, process
lists, installed-package or host inventory, disk encryption state, CPU/memory/
disk/network gauges, user activity, keystrokes, and screenshots.

Enforcement is structural rather than heuristic, which is why the schema is
closed:

1. Every string field has a grammar that cannot express a path or a URL. No P0
   field permits `/`, `\`, `:`, `@`, or NUL, and no P0 field exceeds 128 bytes.
2. Any string beginning with `secret://` is rejected with
   `health_invalid_message` (1102) in every field.
3. Any field name not in the frozen list is rejected with `health_unknown_field`
   (1114), so a new fact cannot be smuggled in.
4. The sender applies the existing centralized redaction before constructing a
   message; the receiver rejects rather than redacts, so a redaction bug cannot
   silently persist sensitive data.
5. Health Plane audit rows record the stable error code, the peer node ID, the
   message kind, and byte counts. They never record payload bytes, field values,
   signatures, or key material.

## Corruption and Migration Failure

Health Plane state is **derived and disposable**. It can be discarded and rebuilt
from subsequent Profiles, Pulses, and Signals. Identity, trust, revocation, and
run history are not derived and are never touched by Health Plane recovery.

| Case | Frozen behavior |
|---|---|
| Schema migration to version 7 | Forward-only, one transaction, updates both `PRAGMA user_version` and the `metadata.schema_version` row atomically, exactly like the existing v1 through v6 migrations |
| Schema migration to version 8 | Forward-only, one transaction, adds `health_profiles.baseline_id` and `health_profiles.baseline_observed_id` defaulted to empty, and fails hard rather than degrading: there is no half-state in which a closed Profile schema requires two fields the storage cannot hold. A node that never completed the version 7 migration stays at version 6 with the plane disabled and never reaches it |
| Migration failure | Full rollback; the database stays at version 6; the node starts with the Health Plane disabled while transport, enrollment, HTTP, and runs continue; a `health_corrupt_state` (1115) audit is written |
| Migration retry | Only on explicit operator action; never automatic on every start |
| Second database | Never created; the Health Plane lives only in `node.sqlite` |
| Rows from schema versions 1 through 6 | Never mutated in place, never dropped by the Health Plane migration |
| Downgrade | A node that finds `schema_version > 7` refuses to start rather than downgrading |
| Single corrupt Health Plane row | Delete only that row, audit `health_corrupt_state` (1115), continue |
| `SQLITE_CORRUPT` on a Health Plane table | Disable the Health Plane for the process lifetime, audit, keep transport and runs serving; never rebuild trust from incoming data |
| Corrupt trust or identity state | The frozen transport behavior applies unchanged: fail closed and preserve evidence |
| Health Plane reset | The operator may drop and recreate Health Plane tables; this never clears trust, revocations, or replay keys used by enrollment |

## Threat Model

| Threat | Frozen response |
|---|---|
| Forged Profile/Pulse/Signal | Every message is BIP-340-signed by the sender identity over its full canonical bytes and carried inside an authenticated Noise session; `sender` must equal the session's authenticated node ID |
| Third-party redirection | `payload.target` is inside the signature and must equal the receiver's active node ID; `health_wrong_target` (1104) |
| Cross-session replay | `session_id` is inside the signature and bound to the Noise final handshake hash; a message from another session fails at step 3 |
| In-session replay | `message_id` replay keys recorded before apply, with a 180-second security floor; `health_replay` (1110) |
| Application replay | `signal_id` idempotency plus a monotonic cursor; a Signal is never applied twice |
| Reordering | Strict `cursor + 1` acceptance with a bounded 32-entry, 60-second reorder buffer; `health_reordered` (1111) |
| Stale or future data | 120-second past and 60-second future windows, inclusive boundaries; `health_stale` (1108), `health_future` (1109) |
| Privilege escalation via message content | Authorization reads only the local `trusted_peers` row; `capabilities` inside a Profile is display-only |
| Revoked node still reporting | Trust is checked at step 7 on every message; `health_revoked` (1107) |
| Wrong-direction message | Role check at step 8; a Performer cannot ack itself and a Conductor cannot report health; `health_wrong_role` (1105) |
| Capability bypass | Capability check at step 9; `health_missing_capability` (1106) |
| Resource exhaustion by flooding | Per-peer rate limits, burst allowance, bounded inbox/outbox, bounded reorder buffer, bounded replay table, and a hard 25,464,832-byte storage ceiling |
| Amplification through error replies | Errors are at most 576 encoded bytes, are sent only to authenticated authorized target-bound peers, and carry only a code and its name |
| Information disclosure | Closed P0 field list, structural P1 rejection, grammars that cannot express paths or URLs, and payload-free audit rows |
| Downgrade to an older schema | Strict `health_version == 1`, no field-omission fallback, no negotiation, refusal to start on a newer database |
| Trust mutation by health data | The Health Plane may write only Health Plane rows and Health Plane audit rows; no code path from a Health Plane message reaches identity, trust, capability, revocation, or run state |
| Health state used as trust | Presence, Profile, and Signals are a projection; they never authorize a session, an enrollment, or future remote work |
| Corrupt or hostile local state | Derived and disposable: quarantine the row or disable the plane; never rebuild trust from incoming data |

## Exclusions

The following are explicitly out of scope for the Health Plane and for plan
`health-plane-foundation`. Implementing any of them requires a new owner-approved
contract.

- Nostr transport, relays, gift wrapping, and any non-direct delivery backend.
- Lua and any embedded script runtime.
- Baselines and baseline push.
- Dashboards, alert engines, notification routing, webhooks, and subscriptions.
- Arbitrary metrics, custom fields, extensible payloads, and any generic event bus.
- Long-term telemetry, time-series history, and any Profile or Pulse history
  beyond the single latest row.
- Signal kinds beyond `enrolled`, `revoked`, and `run-completed`.
- MDM features: lost-device wipe, disk-encryption reporting, package inventory,
  configuration enforcement, and compliance scoring.
- Install automation and unattended provisioning.
- Any change to the frozen direct-transport identity construction, certificate,
  Noise handshake, framing, or enrollment authority.

## Executable Vectors and Feasibility

`tests/fixtures/health_plane_vectors.toml` is the frozen public vector file. It
records the contract identifiers, every bound in this document as a machine-
readable value, the canonical bytes and BIP-340 signature of one accepted message
per kind, and one rejection case per stable error code. Its private values are
published test inputs and are explicitly not production secrets.

`tests/health_plane_contract.rs` is the executable contract. It asserts every
frozen bound against this document's values, verifies the canonical encoding and
signature of each accepted vector with the production
`omakure::direct_transport` signing and verification path, and asserts that each
rejection vector produces its stable error code.

`tests/health_plane_feasibility.rs` is the disposable production-listener
feasibility probe. It starts two real `node serve` processes with the production
direct listener, establishes a real Noise session with the production handshake
and certificate path, sends real Health Plane envelopes over that session, and
proves on the live `node.sqlite` of the running service that role and capability
authorization is decidable for every kind. It adds no shipped Health Plane
surface: the production listener has no Health Plane handler yet, so the probe
asserts the current bounded rejection and the availability of the authorization
inputs, not a Health Plane response.

### Production carriage feasibility

The shipped production listener has no application dispatcher: `serve_connection`
in `src/direct_service.rs` hardcodes the single expected first envelope kind
`probe`, replies with `ack`, and then enters `hold_session`, which decrypts every
later application frame and discards its plaintext. A Health Plane message
therefore already traverses the complete production path today - outer frame,
session ID, Noise transport decryption, inner sequence check - and is dropped
without any state mutation.

Implementation of the Health Plane consequently requires exactly these
production seams, and no others:

1. New `sign_health_*` wrappers around the existing private `sign_envelope` in
   `src/direct_transport.rs`, plus one public accessor that reads the envelope
   `kind` string so a receiver can dispatch before calling `verify_envelope`.
   The signing construction itself is unchanged.
2. A kind dispatch inside `hold_session` in `src/direct_service.rs`, which is the
   single shared steady-state receive loop for both connection directions.
3. The read-only role/capability projection described in Authorization Mapping.
4. The schema version 7 migration and the Health Plane tables, and the version
   8 migration that widened the stored Profile.

No change to the certificate, the Noise handshake, the frame format, the inner
control kinds, the admission controller, or the enrollment path is authorized.

## Go/No-Go Gate

This contract is the wave-1 gate for plan #190. Downstream tasks 2777, 2778,
2779, and 2780 may not begin until:

1. The owner has reviewed and accepted this document.
2. `cargo test` and `cargo clippy --all-targets -- -D warnings` plus
   `cargo fmt --check` are green.
3. The vectors and the feasibility probe are green.

If any bound in this document proves unenforceable during implementation, the
plan stops and this contract is amended before work resumes. An implementation
task MUST NOT invent a bound that is not written here.

## References

- `.docs/direct-transport-contract.md` - frozen transport, certificate, envelope,
  enrollment, and registry contract.
- `rebuild-omakure.md` - roadmap definitions of Conductor, Performer, Pulse,
  Profile, Signal, and the node foundation threat model.
- [RFC 8785 JSON Canonicalization Scheme](https://www.rfc-editor.org/rfc/rfc8785)
- [BIP-340 Schnorr Signatures](https://github.com/bitcoin/bips/blob/master/bip-0340.mediawiki)
