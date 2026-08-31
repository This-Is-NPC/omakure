# Signed Bundle: Docker

**Status:** Ignored/manual; selected by the Linux transport certification and packaged-image CI covers related enrollment paths.

## Source

- `tests/docker_signed_bundle_e2e.rs`
- `ci/compose/compose.signed-bundle.e2e.yaml`

## Run

```bash
cargo test --test docker_signed_bundle_e2e --locked -- --ignored --nocapture docker_signed_bundle
```

## Proves

- An isolated authority and fresh targets complete signed-bundle enrollment with replay-safe bootstrap material.
- Enrollment is bound to the expected audience/authority and remains stable across restart.
- An autojoin target can join with no command run on that machine; it receives no pre-mounted bundle.
- Cleanup after partial Compose startup is tested separately.

## Does Not Prove

- It does not certify arbitrary authority operations or production certificate rotation policy.
- The unattended autojoin case is not a general claim that every installer path is automated.

## Topology and Cleanup

The Compose file separates authority, target-a, target-b, and optional autojoin
config/token/state volumes. Bundle files are mounted read-only where applicable;
autojoin deliberately has no bundle mount. Tests use bounded Compose/Docker calls
and remove project resources in cleanup.

## Troubleshooting

- Autojoin does not start: verify the `autojoin` profile inputs and authority public configuration, not a missing bundle file.
- Replay accepted: preserve only the safe error code and inspect the target registry/audit state.
