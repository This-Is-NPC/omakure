# Deploy Policy

**Status:** CI.

## Source

- `tests/policy_e2e.rs`
- Policy loading and enforcement: `src/policy.rs` and the shared operations layer.

## Run

```bash
cargo test --test policy_e2e --locked
```

## Proves

- A read-only policy blocks wildcard-token writes.
- Disabled Battery routes are denied.
- Legacy-disabled and malformed policy files fail before the listener binds.
- Non-loopback binding requires explicit opt-in; policy can supply that opt-in.
- Worker defaults are taken from policy and are observable through node-service behavior.

## Does Not Prove

- It does not validate an external policy deployment or authorization proxy.
- It does not replace per-route HTTP capability coverage.

## Environment and Cleanup

Each case uses a temporary policy file, workspace, token, and bounded process
startup. Listener-bind assertions use an available loopback port and verify the
port is not reachable after startup rejection.

## Troubleshooting

- Startup unexpectedly succeeds: check policy precedence and `OMAKURE_POLICY_FILE`.
- A write is denied: inspect the effective policy rather than broadening the token first.
