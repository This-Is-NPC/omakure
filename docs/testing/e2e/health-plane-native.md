# Health Plane: Native

**Status:** CI; native cases provide platform coverage where Docker networking is unavailable.

## Source

- `tests/health_plane_contract.rs`
- `tests/health_plane_state.rs`
- `tests/health_plane_signals.rs`
- `tests/health_plane_transport_e2e.rs`
- `tests/health_plane_feasibility.rs`

## Run

```bash
cargo test --test health_plane_contract --test health_plane_state \
  --test health_plane_signals --test health_plane_transport_e2e \
  --test health_plane_feasibility --locked
```

## Proves

- Frozen Profile/Pulse/Signal contracts, schema migration, durable replay protection, retention, revocation, and redacted public projections.
- Real production Noise sessions carry Profile and Pulse messages between nodes.
- CLI and HTTP fleet projections agree on presence/status.
- Signal delivery is idempotent across duplicates and restarts; injected clocks test exact freshness, reorder, rate, and retention boundaries without sleeping.
- Raw adversarial messages are dropped or answered according to authentication/authorization/target rules, audited with stable codes, and cannot mutate trust/state.

## Does Not Prove

- Native coverage does not prove the packaged Docker network; see [Health Plane Docker](health-plane-docker.md).
- The feasibility probe is a disposable listener check, not a production deployment certification.

## Bounds and Cleanup

Real reachability uses 45 seconds, admission-window retries 90 seconds, and
three-node fleet reachability 110 seconds. Presence windows are tested with
injected clocks rather than ten-minute sleeps. Services and temporary registries
are stopped and removed by the suite.

## Troubleshooting

- Stale/online timing failure: check the frozen contract constants before changing a wait.
- Adversary receives a response when it should be dropped: inspect the message's authentication, authorization, target, and audit-code path.
