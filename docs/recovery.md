# Node Recovery

Node identity and trust state are machine-owned and separate from the script
workspace. Preserve the node state volume during routine restarts, image
updates, and workspace replacement.

## Normal restart

Restart the same `node serve` deployment with the same node-state path or
volume. Readiness must return after the configured static peers reconnect.

```bash
omakure node status --json
omakure node peers --json
curl http://127.0.0.1:7878/v1/ready
```

## Revocation

Revoke a peer through the authenticated node operation with an explicit actor,
reason, and confirmation. Revocation is local durable state; transport
handshakes cannot restore trust.

```bash
omakure node revoke NODE_ID --actor operator --reason "device retired" --confirmed
```

Inspect the node audit output after the operation and confirm the revoked peer
cannot establish a useful direct session. Do not delete the node database to
work around a revocation.

Revocation also reaches work that peer already caused: queued and running
Cue-origin runs are cancelled, and the executor heartbeat kills the child as
soon as the row leaves `running`. It is scoped to `trigger = 'cue'` — revoking
a peer is not a licence to cancel what this node's own owner started. The
revocation itself is written first and is never made conditional on the run log,
so withdrawing trust cannot be blocked by a busy or missing workspace database.

It reaches the transport too, and no restart is required for any of it:

- **A standing session ends** within about a second. `node status` stops
  reporting the peer `connected`, which is what makes the check above answerable
  at all.
- **A Cue or a baseline push aimed at that peer is refused** by this node before
  anything reaches the wire, naming the peer and the state it was found in. Do
  not read this as the peer having refused; it never heard the request.
- **The peer's own reconnects are refused with `revoked`**, which it is told, so
  its dialer stops rather than retrying. Expect `revoked` — not `internal` — in
  that node's `node status` under `transport.last_errors`.

The asymmetry is deliberate and is worth expecting: the revoked node is never
told it was revoked until it next connects, so until then it still lists the
revoker as an active peer. That is its registry being honest about what it
knows, not a failed revocation.

## A worker that died holding a remote run

A Cue-origin run is deliberately excluded from the worker lease steal. Where an
ordinary abandoned run is re-claimed and re-executed after the heartbeat lapses,
a remote instruction must execute at most once, so its row is instead resolved
to `failed` by the recovery pass that runs at worker startup. Expect to find:

```bash
omakure history list --state failed
```

with the error `the worker holding this remote run stopped; it was not re-run
because a remote instruction must execute at most once`. Re-dispatch it
deliberately if it should happen again; nothing will do so on its own. Use
`--cue-id` to retry the same instruction; a new id is a new run.

## A machine that did not join

A provisioned machine that never joined its fleet is serving and reachable —
that is deliberate, because a node that refused to boot could not be asked
anything. Check it in this order:

```bash
omakure node status          # identity present? peer count still zero?
omakure node peers           # nothing active means the bundle never landed
```

The delivery is a request, so the fleet that pushed the bundle has the typed
refusal in hand. Read it there first: unknown or revoked authority, wrong
organization, expired, replayed, or enrollment not enabled.

The two that look alike and are not:

- **`enrollment_disabled`** — the target's `trust.enrollment` is not
  `signed-bundle`. The shipped default is `disabled`, so this is what a
  provisioning mistake looks like: the machine is healthy and was never told to
  accept membership.
- **`enrollment_replay`** — the bootstrap token was already spent. The machine
  most likely joined already; check `node peers` before reissuing anything.

Reissue rather than reuse. A bundle is bound to one node id and one bootstrap
pair, so a second delivery of the same bundle is refused by design.

## A machine that drifted

`omakure node health --json` on the Conductor answers this, and the answer is a
comparison rather than a claim. A Performer reports the identity of the set it
recorded installing beside the identity of that same set as it is on its disk
now; the projection compares them and reports `drifted` when they differ.

```bash
omakure node health --json | jq '.data.baselines'
omakure node health --json | jq '.data.nodes[] | select(.baseline_status == "drifted")'
```

Read the two states that are not drift before reaching for a fix:

- **`unknown`** — that Performer has not reported a Profile yet. It is not a
  statement about a baseline; check presence first.
- **`none`** — it reported holding no baseline at all. It was never pushed one.
  Push one; there is nothing to repair.

Drift means the set that was installed is no longer what was installed —
someone edited, replaced, or deleted a script the baseline named. A file that
*no* baseline entry names is not part of the published set and does not make a
machine drift; the identity is a hash of the set that was signed.

Two ways back, and they answer different questions.

**The set is right and the machine is wrong.** Push the same baseline again from
the Conductor. It reinstalls the whole set or none of it, and the machine is in
sync again on the next Profile.

```bash
omakure node baseline push --peer-node-id omk1_... --manifest ./baseline-v1.omb
```

**The set is wrong.** Put the machine back on the version before it, on the
machine itself:

```bash
omakure node baseline rollback --confirmed
curl -X POST -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  -H 'content-type: application/json' --data '{"confirmed":true}' \
  http://127.0.0.1:7878/v1/node/baseline/rollback
```

What to expect:

- The restored bytes are the **signed** ones this node retained, not whatever
  was on disk when the next push replaced them, so a rollback also undoes drift
  in the version it restores.
- Exactly one previous version is kept, and this is a **swap**: the version
  rolled away from becomes the retained previous, so rolling back twice returns
  the machine to where it started.
- `not_found` means nothing is retained. A node that has only ever been pushed
  one baseline has no previous version, and refusing is the answer — there is
  nothing to put back.
- `forbidden` with `baseline_publisher_revoked` means the publisher that signed
  the retained set is no longer one this node accepts code from. That is the
  rollback doing its job: it is re-verified against today's publishers, so it
  cannot walk a machine back onto code the fleet has disowned. Sign a
  replacement under a current key and push it.
- `forbidden` with `baseline_expired` on a rollback means the retained record
  claims to have been installed before its own manifest was issued. An ordinary
  expired manifest does **not** block a rollback: the validity window is
  answered as of the instant this node accepted the set, because nothing is
  being delivered.

A refused rollback changes nothing. The machine stays on the baseline it was
running, and the fleet view is unchanged.

## Identity replacement

Use `node reset --confirmed` only when the machine identity and trust registry
must be destroyed. Stop the service first, preserve any required audit export,
reset the node state, and restart to generate a fresh identity. Re-enroll the
replacement explicitly; the old identity must not be silently reused.

## Certification recovery evidence

The bounded Linux recovery path is exercised by:

```bash
mise run cert:transport
```

That gate verifies partition/reconnect, revocation, reset/replacement, durable
audit records, and cleanup of all temporary Docker volumes. It does not claim
Nostr, campaigns, or MDM behavior.

Health Plane recovery has its own bounded Linux gate:

```bash
mise run cert:health
```

That gate verifies the recovery paths an operator actually depends on: a
Performer that goes `stale` while isolated and returns `online` with fresh Pulse
state after rejoining; Health Plane state surviving a Conductor restart with an
unchanged identity, unchanged persisted key material, and a non-regressing audit
trail; a corrupt stored Profile row being quarantined with the frozen
`health_corrupt_state` (1115) audit rather than served, followed by the
Performer re-reporting a fresh Profile; revocation excluding a Performer from
the fleet immediately and the bounded retention pass purging its Health Plane
rows; and a replaced identity rejoining with a fresh Signal cursor while the old
identity holds no Health Plane state. It does not claim Nostr or campaigns;
baseline push, drift, and verified rollback are proved separately on the
packaged image by the `docker-smoke` CI job.
