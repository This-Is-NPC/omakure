# Remote Cue: Native

**Status:** Ignored/manual for the two-real-node suite; packaged-image Cue coverage runs in Linux CI.

## Source

- `tests/remote_cue_e2e.rs`
- Contract/authorization support: `tests/remote_cue_contract.rs`, `tests/remote_cue_authorization.rs`

## Run

```bash
cargo test --test remote_cue_e2e --locked -- --ignored --nocapture
```

## Proves

- Two real `node serve` processes exchange an authorized Cue over Noise and execute the declared script exactly once.
- A second dispatch with the same `--cue-id` does not run the script twice.
- The service path is used when a standing session already exists.
- A fully trusted conductor cannot run an undeclared performer script.
- Cue/run correlation and outcome Signal handling are asserted through operator-facing CLI state.

## Does Not Prove

- Cue idempotency is not a general exactly-once guarantee for arbitrary external side effects; the test counts a local marker effect.
- The suite is ignored because it starts real services and is not a default CI guarantee. Packaged CI covers one image-level scenario, not every native case.

## Bounds and Cleanup

The effect wait is 30 seconds and Signal wait is 90 seconds. Standing-session
polling is 30 seconds. Temporary conductor/performer workspaces and services
are dropped after each case.

## Troubleshooting

- Direct fallback was used: wait for the standing session and inspect `via`; dispatching too early tests the wrong path.
- Duplicate marker effect: inspect service logs and run history, but do not describe this as arbitrary external-effect exactly-once.
