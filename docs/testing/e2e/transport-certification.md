# Transport Certification

**Status:** Certification; Linux CI and release gates.

## Source

- `scripts/tasks/cert/transport`
- `ci/compose/compose.transport-certification.e2e.yaml`
- `tests/direct_transport_e2e.rs`, `tests/docker_discovery_e2e.rs`, `tests/docker_enrollment_e2e.rs`, `tests/docker_signed_bundle_e2e.rs`

## Run

```bash
scripts/tasks/cert/transport
scripts/tasks/cert/transport-cleanup
```

CI wraps the canonical gate with `timeout --foreground --kill-after=15s 20m`.
The release workflow runs the same gate and requires it before publishing.

## Proves

- The current packaged image initializes four isolated certification principals, generates scoped multi-token auth, and uses real direct TCP ingress.
- Accepted/rejected encrypted probes, certificate/identity/target checks, reconnect, revocation, discovery follow-up, manual enrollment, and signed-bundle enrollment all pass through the declared topology.
- Transport audit rows are durable and redacted; bearer tokens, Argon2 hashes, and endpoints do not appear in logs/audits.
- Cleanup is verified for containers, networks, and volumes after normal and induced-failure paths.

## Does Not Prove

- It is Linux/Docker certification, not a cross-platform network guarantee.
- ARM jobs are workflow evidence until those jobs run; this certification does not silently credit ARM.
- The Compose file is ephemeral and must not be used as a persistent deployment sample.

## Bounds and Cleanup

Compose and Docker commands are bounded at 120 seconds. Service readiness is 45
seconds, connection 90 seconds, disconnection 75 seconds, and the aggregate
Rust/Docker phases are bounded by the script. Cleanup always runs and fails if
resource inspection fails or finds leftovers.

## Troubleshooting

- Token generation fails: inspect only the safe failure classification; the script refuses to print sensitive token material.
- Dial timeout: allow one full retry-backoff ceiling before changing the bound; the script's 90-second wait is intentional.
