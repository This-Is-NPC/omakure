# Discovery: Docker

**Status:** Ignored/manual; selected by the Linux transport certification.

## Source

- `tests/docker_discovery_e2e.rs`
- `scripts/tasks/cert/transport`

## Run

```bash
cargo test --test docker_discovery_e2e --locked -- --ignored --nocapture docker_discovery
```

The canonical aggregate invocation is bounded by the transport certification
script and selects the `docker_` tests after bringing up its topology.

## Proves

- A candidate is discovered over the Docker/Linux bridge without creating trust or a transport session.
- Discovery candidates do not expose addresses when address disclosure is disabled.
- A discovery result does not authorize a direct probe.
- Explicit manual enrollment can then be requested and approved, proving discovery remains separate from enrollment.

## Does Not Prove

- Docker broadcast/multicast support is host- and daemon-dependent.
- Discovery alone never proves authorization or connectivity.

## Bounds and Cleanup

Discovery polling uses a 15-second candidate deadline and bounded health/HTTP
calls. The test's Compose guard removes volumes and orphans. The certification
script performs a second project-labeled resource inspection.

## Troubleshooting

- Candidate absent: verify Docker bridge broadcast/multicast and inspect the Compose logs.
- Probe succeeds after discovery: treat it as a trust-boundary regression; discovery must leave registry counts at zero.
