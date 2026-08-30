# Health Plane: Docker

**Status:** Certification in Linux CI; adversarial Rust cases are ignored and selected explicitly by the gate.

## Source

- `.scripts/health-plane-certification.sh`
- `compose.health-plane-certification.e2e.yaml`
- `tests/docker_health_plane_adversary.rs`
- `tests/docker_health_plane_exhaustion.rs`
- `Dockerfile` (`harness` target for `hp-harness`)

## Run

```bash
./.scripts/health-plane-certification.sh
./.scripts/health-plane-certification-cleanup-test.sh
```

The CI job bounds the gate at 45 minutes, the cleanup test at 60 minutes, and
the whole job at 120 minutes. The two adversarial phases are different:

- `hp-harness` is started by the script as a container from the Dockerfile's
  `harness` target. That image contains the compiled
  `docker_health_plane_exhaustion` test binary and its default command selects
  `an_unacknowledged_profile_stops_at_the_frozen_attempt_budget_on_one_session`.
  The repointed Performer dials this listener over `hp-net`.
- The contracted adversarial matrix is separately invoked by the script as a
  host-side `cargo test --test docker_health_plane_adversary --locked --
  --ignored --nocapture` process, using the live topology and published direct
  listener. It does not run inside `hp-harness`.

## Proves

- Four independently stateful packaged nodes exchange Health Plane data only over production Noise on direct port 7879; management HTTP is loopback-only and read-only.
- Runtime node-ID ranking makes dial ownership deterministic; the adversary is not trusted by the fleet.
- Profile/Pulse reachability, presence transitions, Signal delivery, replay/rate/freshness/size/schema/target rejection, audit codes, and redaction are exercised.
- The `hp-harness` exhaustion phase keeps one real Noise session open, withholds acknowledgements, and proves the Performer stops Profile attempts at the frozen retry budget.
- The separately invoked `docker_health_plane_adversary` matrix drives contracted wrong-target, freshness, schema, size, authorization, replay, and related cases over production Noise; it proves neither that matrix nor the exhaustion phase can mutate trust or derived state.

## Does Not Prove

- It does not prove a host-wide multi-network deployment or external load balancer behavior.
- A passing gate certifies this topology and artifact, not arbitrary operator Compose files.

## Bounds and Cleanup

Frozen budgets include 90 seconds readiness/connect, 110 seconds report reach,
200 seconds stale observation, 180 seconds maintenance, 30 seconds audit, and
the documented 60-second admission/retry windows. The script gives the
host-side adversary matrix a 20-minute timeout with a 30-second kill-after,
allows 90 seconds for the `hp-harness` readiness marker, and bounds waiting for
the harness container at 6 minutes with a 15-second kill-after. Cleanup handles
EXIT/INT/TERM, runs `down --volumes --remove-orphans`, reclaims bind-mounted
ownership, and fails closed if any project container/network/volume remains.
The interrupt handler is reliable for SIGTERM; automation should prefer SIGTERM
over a shell background SIGINT.

## Troubleshooting

- No node-to-node messages: inspect direct 7879 and node-ID ranking; management HTTP cannot be the data path by topology.
- Resource remains: do not report success; capture safe logs, rerun the cleanup test, and inspect only the project-labeled resources.
