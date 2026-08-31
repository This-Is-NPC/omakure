# Baseline Delivery Contract

**Status: IMPLEMENTED CONTRACT.** This document records the wire format,
authorization mapping, and every quantitative bound implemented for putting a
signed baseline onto a node, keeping one version behind it, and putting a
machine back. It is the delivery half; the signing half is `src/baseline.rs`
and `src/baseline_publisher.rs`.

Every number here is normative. A limit not written here requires an amendment
to this document before it can be used.

## What is different about this plane

A Cue "names a script and never carries one", and that single sentence carries
most of the Remote Cue plane's safety: the worst a fully compromised Conductor
can do is run something the node's owner already put in the workspace.

A baseline carries code. That argument is not available here, and nothing in
this document should be read as if it were. The question this plane has to
answer is not "what would an attacker have to already possess" but "what would
have to fail, all at once, for arbitrary code to land".

## Two independent authorities

A baseline installs only if **both** hold, and neither substitutes for the
other:

| Authority | What it proves | Where it is read |
|---|---|---|
| The sender is an active Conductor holding `baseline-push` | Someone this node agreed to take orders from is asking | The receiver's own trust registry |
| The manifest verifies under a publisher in `trust.baseline_publishers` | The bytes are the set someone this node named signed | The receiver's own config |

Compromising the Conductor's session key does not produce a publisher
signature. Holding the publisher key does not produce a session with a node
that does not trust you.

`baseline-push` was already in the frozen capability allow-list
(`src/health_plane/bounds.rs`). No capability is added.

`node_registry` refuses to let one node hold a publisher key *and* conduct
anyone, in both directions. A node able to author code and order every
Performer to run it is that separation gone.

## Version and Domains

| Item | Value |
|---|---|
| Kind prefix | `baseline_` |
| Kinds | `baseline_push` (Conductor → Performer), `baseline_ack` (Performer → Conductor) |
| Signature | BIP-340/secp256k1 over the direct envelope, unchanged |
| Envelope domain | `omakure/direct-envelope/v1\0`, unchanged |
| Manifest domain | `omakure/baseline-manifest/v1\0` |
| Baseline id domain | `omakure/baseline-id/v1\0` |
| Script hash domain | `omakure/baseline-script/v1\0` |

`sign_baseline_envelope` is a third sibling to `sign_health_envelope` and
`sign_cue_envelope` over the same private, kind-agnostic `sign_envelope`. Each
refuses every kind outside its own namespace, so none can become a signing
oracle for another's plane. The inner frame is unchanged.

## `baseline_push` Payload

| Field | Type | Bound |
|---|---|---|
| `version` | integer | Exactly `1` |
| `manifest` | string | Lowercase hex of the signed manifest, at most `2 × 65,536` chars |
| `scripts` | array of string | Lowercase hex bodies **in manifest order**, at most `256` entries, at most `262,144` raw bytes in total |

**No paths travel on the wire.** Where each script goes comes from the signed
manifest and nowhere else, so a sender cannot name a destination the publisher
did not sign — not a rejected one, because there is no field in which to say
it.

Bodies are matched to entries positionally against the manifest's own sorted
entry list. The receiver checks the array length against the entry count before
zipping; a short array is `content_mismatch`, not a shorter set.

## `baseline_ack` Payload

| Field | Type | Bound |
|---|---|---|
| `version` | integer | Exactly `1` |
| `baseline_id` | string | 64 lowercase hex chars, computed by the *receiver* from the manifest it decoded |
| `accepted` | boolean | Whether the baseline installed |
| `error` | object | Present only on a reportable refusal: `{ "code": <u16> }` |

The `baseline_id` is recomputed rather than echoed, so an ack can only ever be
an answer about the set the receiver actually evaluated.

## Size: why delivery carries its own bound

The frozen Noise plaintext limit is `MAX_PLAINTEXT_BYTES` = **1,048,520**
bytes (`docs/internal/direct-transport-contract.md`). A signable baseline may hold
`MAX_ENTRIES` = 256 scripts of `MAX_SCRIPT_BYTES` = 1 MiB each, so a maximal
baseline is roughly **256 MiB**. It does not fit in one frame, and not by any
margin.

Three answers were considered:

1. **Raise the frame limit.** Refused. The 1 MiB bound is frozen, is load-
   bearing for the queue and per-source byte budgets, and would have to grow by
   more than two orders of magnitude to make the worst case fit.
2. **Chunked reassembly.** It is not part of this contract. It needs a
   multi-frame state machine, a receive buffer sized to the largest baseline,
   abort and resume states, and a new way for a peer to make a node hold
   megabytes — every one of which is new attack surface on the one plane that
   carries code.
3. **A smaller delivery bound.** Taken.

`MAX_PUSH_SCRIPT_BYTES` = **262,144** raw bytes of script content per push.
Hexed that is 512 KiB; a maximal manifest adds 128 KiB hexed; the envelope and
JSON scaffolding add a few hundred bytes. The total is comfortably inside the
frozen limit, with headroom rather than exactly at it.

Enforced in three places:

- **At publish**, so an operator learns a set cannot be delivered before
  signing it rather than by trying to send it.
- **At send**, before any of it goes on the wire.
- **At receive**, on declared length before any decoding allocates.

The shipped `omakure node baseline publish` operation rejects a set over that
bound, before signing or writing a manifest, so every baseline produced by the
CLI is deliverable in one push. `src/baseline_push.rs` carries a `const`
assertion that breaks the build if a maximal schema-valid baseline ever *does*
fit in one frame, because at that point this delivery bound would be an
arbitrary restriction rather than a consequence.

## Rejection Codes

Band `1301..=1399`, disjoint from transport (`1001..=1020`), Health
(`1101..=1115`) and Cue (`1201..=1212`).

| Code | Name | Reportable |
|---|---|---|
| 1301 | `baseline_disabled` | No |
| 1302 | `baseline_not_active_conductor` | No |
| 1303 | `baseline_missing_baseline_push` | No |
| 1304 | `baseline_invalid_message` | Yes |
| 1305 | `baseline_too_large` | Yes |
| 1306 | `baseline_publisher_unknown` | Yes |
| 1307 | `baseline_publisher_revoked` | Yes |
| 1308 | `baseline_organization_mismatch` | Yes |
| 1309 | `baseline_expired` | Yes |
| 1310 | `baseline_signature_mismatch` | Yes |
| 1311 | `baseline_content_mismatch` | Yes |
| 1312 | `baseline_install_failed` | Yes |
| 1313 | `baseline_duplicate` | Yes |

The three unreportable codes are the Health and Cue precedent unchanged:
whether this node has the feature on, and what it thinks of the *sender*, are
never disclosed. An unauthorized peer must not learn that baseline push exists
here. Everything else is about the artefact the sender chose, and an authorized
Conductor that is not told why its push failed can only guess.

## Gate Order

Order is load-bearing, and it is not observable from whether the install
happened — the sender's standing is read twice.

1. Envelope verifies against the handshake identity and this session id.
2. **Sender standing**: gate on, active Conductor, `baseline-push` held.
3. Payload shape and size.
4. Manifest decodes; the publisher key id is one this node named.
5. Publisher not revoked; organization matches; inside the validity window;
   signature verifies.
6. Every script body matches its recorded hash (`bind`, all or none).
7. **Sender standing, re-read**, immediately before the write.
8. Install.

Step 2 is what stops an untrusted peer ever reaching a signature verification
and learning what this node thinks of a publisher. Step 7 is TOCTOU: steps 4–6
walk a signature and hash every script, and a peer revoked while that ran must
not have its code installed. Revocation that only affected the reply would be
advisory.

## Install

`bind` is the only route to a baseline's bytes and is deliberately not
incremental, so a partial *verification* is unreachable. The filesystem has no
such property, so all-or-nothing is built: every write is staged with the state
needed to undo it, and one failure walks the successful ones back.

The set record (`.omakure/baseline.json`) is written last and unwound with the
scripts, so "the scripts are installed" and "this node holds this baseline"
cannot disagree.

Path confinement is re-applied at install time rather than trusted from
`validate_entry_path` at signing time. A check that only holds when someone
else remembered to run it is not a boundary.

**Battery provenance is deliberately not reused.** Four of its fields — git
URL, requested ref, resolved commit, source path — have no meaning for a
baseline. More seriously, `battery::installing_battery` scans that directory to
answer gate E of the Remote Cue plane: a baseline script recorded there would
read as battery-installed, and a node whose `trust.remote_cue_batteries` named
that battery would silently have made it remotely runnable.

## Refusing a baseline is not refusing to serve

`docs/fleet-operations.md` freezes this rule for enrollment and it applies unchanged. Every
refusal above is a decision about one message. The session stays up, the Health
Plane keeps reporting, and the next baseline on the same session is decided
normally.

## Carriage

Delivery uses the existing outbox on `ConnectionState`, which also carries Cues,
because a node holds one session per peer and a separate process cannot dial a
peer the running service is already connected to. `POST /v1/node/baselines`
exposes it under the existing `node:write` scope; no new `ApiCapability` was
needed. `omakure node baseline push` calls that route.

There is **no direct-dial fallback**, unlike `node cue`. A Cue has a
first-contact case; a baseline goes to a Performer this node already conducts,
which is exactly the peer it already holds a session with. A dial would have
been a second way into the responder for a case that does not exist.

The service never signs a manifest and holds no path to a publisher key. The
operator signs where the key is; the service carries what it is given.

## Consequence for the Remote Cue contract

A baseline replaces scripts *legitimately*. A Cue accepted against version N of
a script could therefore execute N+1 with no attacker anywhere in the story.
`docs/internal/remote-cue-contract.md` declined an exec-time content re-check on the
grounds that it only defended against an attacker who could already write to
the workspace; this plane makes that premise false, and the check is now live
and fail-closed. `hash_reverified_at_exec = true`.

## Retention and Rollback

The plane has two message kinds and no third: **rollback is local.**

### What "previous" means

Exactly **one** previous baseline is retained, in
`.omakure/baseline-previous.json`, beside the one currently installed in
`.omakure/baseline-current.json`. Each slot holds a `baseline_push` payload
verbatim — the signed manifest and the script bodies in manifest order — plus
the Unix second this node accepted it.

| Item | Value |
|---|---|
| Retained versions | Exactly 1 previous, plus the current |
| Current slot | `.omakure/baseline-current.json` |
| Previous slot | `.omakure/baseline-previous.json` |
| Worst-case retained bytes | `2 × (2 × MAX_PUSH_SCRIPT_BYTES + 2 × MAX_MANIFEST_BYTES)` = **1,310,720**, hexed |

The bound is a consequence rather than a new number: every installed baseline
arrived through a push, so its scripts are already capped at
`MAX_PUSH_SCRIPT_BYTES` and its manifest at `MAX_MANIFEST_BYTES`.

Installing rotates: the set being replaced moves into the previous slot, and the
arriving set becomes current. **Rolling back is therefore a swap, not a step
down a stack** — the version rolled away from becomes the retained previous, so
a mistaken rollback can itself be undone and a second rollback returns the
machine to where it started. Deeper history was refused: it needs an operator
vocabulary for *which* version — a name, an index, a listing — that the contract
does not provide and that grows with every push.

A node with nothing in the previous slot refuses with `not_found`. It does not
reinstall what it is already running and report success.

### As verified as the push that installed it

The retained payload goes back through **the same `verify_push`** the delivery
path runs, against the policy this node reads from its own config **now**:

- the publisher key id must still be one `trust.baseline_publishers` names,
- that publisher must not have been revoked since,
- `organization.id` must still match,
- the manifest signature must still verify,
- every retained script body must still match its recorded hash (`bind`, all or
  none).

A rollback to an unsigned or no-longer-verifiable state would launder code past
the publisher check, which is the check this whole plane exists for. A publisher
revoked since the original install makes the rollback fail.

### Validity window at acceptance

**The validity window is evaluated at the instant this node accepted the
baseline, not at rollback time.** The window bounds how long a published
artefact may be *delivered* — it stops a captured push being replayed onto a
machine months later. Nothing is delivered by a rollback: the bytes never leave
the disk they are already on, and this node already accepted them once, inside
that window. Re-evaluating it at rollback time would make rollback useless in
exactly the situation it exists for — a bad push discovered after the previous
manifest's 30-day lifetime ran out, with the publisher offline.

The recorded instant is clamped to now, so a retained record cannot reach
forward into a window that has not opened.

The consequence is stated rather than hidden: **there is no way for a publisher
to retire one specific baseline.** Expiry is time-based and revocation is
key-wide, and neither expresses "not that one". A publisher that needs a version
gone revokes the key and re-signs under a new one — and that does block the
rollback, because revocation is re-read.

### Why rollback is not a third message kind

`omakure node baseline rollback --confirmed` and
`POST /v1/node/baseline/rollback` under `node:write`. Both act on the machine
they run on and reach no peer.

A `baseline_rollback` kind would hand a Conductor the power to flip a Performer
between two code versions at will, without a publisher signature anywhere in
*that* message. The split between publishing and conducting exists to withhold
exactly that. The Conductor already learns which machine needs attention: the
Health Plane's `baseline_status` says `drifted`, and `docs/recovery.md` says
what to do about it.

## Drift

A Performer reports two facts on its Profile and no verdict: `baseline_id`, the
derived name of the set it recorded installing, and `baseline_observed_id`, the
same derivation recomputed over that set's paths as they are on disk now. The
Conductor's fleet projection compares them. The grammar, the four verdicts, and
what the pair does not cover are frozen in `docs/internal/health-plane-contract.md`.

## Out of scope

Campaigns and fan-out. Chunked delivery of a baseline larger than one push.
Retaining more than one previous version.
