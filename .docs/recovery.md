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

## A worker that died holding a remote run

A Cue-origin run is deliberately excluded from the worker lease steal. Where an
ordinary abandoned run is re-claimed and re-executed after the heartbeat lapses,
a remote instruction must execute at most once, so its row is instead resolved
to `failed` by the recovery pass that runs at worker startup. Expect to find:

```bash
omakure runs list --state failed
```

with the error `the worker holding this remote run stopped; it was not re-run
because a remote instruction must execute at most once`. Re-dispatch it
deliberately if it should happen again; nothing will do so on its own.

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

## Identity replacement

Use `node reset --confirmed` only when the machine identity and trust registry
must be destroyed. Stop the service first, preserve any required audit export,
reset the node state, and restart to generate a fresh identity. Re-enroll the
replacement explicitly; the old identity must not be silently reused.

## Certification recovery evidence

The bounded Linux recovery path is exercised by:

```bash
mise run transport-certification
```

That gate verifies partition/reconnect, revocation, reset/replacement, durable
audit records, and cleanup of all temporary Docker volumes. It does not claim
Nostr, campaigns, or MDM behavior.

Health Plane recovery has its own bounded Linux gate:

```bash
mise run health-plane-certification
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
identity holds no Health Plane state. It does not claim Nostr,
baselines, campaigns, or MDM behavior.
