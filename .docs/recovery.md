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
Nostr, remote Cues, campaigns, or MDM behavior.

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
identity holds no Health Plane state. It does not claim Nostr, remote Cues,
baselines, campaigns, or MDM behavior.
