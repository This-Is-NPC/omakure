# Baseline: Native

**Status:** Ignored/manual for two-real-node delivery; packaged-image delivery is exercised in Linux CI.

## Source

- `tests/baseline_push_e2e.rs`
- Baseline delivery implementation and contract tests in `src/baseline_push.rs` and related modules.

## Run

```bash
cargo test --test baseline_push_e2e --locked -- --ignored --nocapture
```

## Proves

- A third publisher signs a manifest, a conductor sends it over a standing session, and a performer installs it.
- The conductor receives an accepted `baseline_ack` and reports the same baseline ID.
- A zero-second caller budget records a late/missed acknowledgement accurately while the performer may still install the baseline.
- Baseline audit rows distinguish delivery, acceptance, and late acknowledgement outcomes.

## Does Not Prove

- It does not prove every scheduler or fleet rollout policy.
- Ignored native tests are not a default CI guarantee.

## Bounds and Cleanup

Standing-session setup is bounded at 30 seconds. The normal push waits up to 60
seconds; late-install observation is bounded at 30 seconds. Temporary publisher,
conductor, performer workspaces and services are dropped.

## Troubleshooting

- `answered: false` with an installed baseline: inspect the conductor's baseline audit row before retrying; it may be a late acknowledgement.
- No connection: ensure both sides have the peer locator because dial ownership is determined by node ID ordering.
