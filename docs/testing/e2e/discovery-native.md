# Discovery: Native

**Status:** CI on Unix; adversarial real-UDP coverage is Unix-gated.

## Source

- `tests/discovery_udp_e2e.rs`
- Discovery implementation and limits in `src/discovery.rs`

## Run

```bash
cargo test --test discovery_udp_e2e --locked
```

## Proves

- A real UDP receiver accepts a valid beacon and drops bad signatures, wrong secrets, truncated/oversized datagrams, and malformed floods.
- Candidate state remains bounded by the configured candidate/source limits.
- Dropped candidates do not gain addresses or create trust/session state.
- Stopping the service removes its listening state.

## Does Not Prove

- It does not prove broadcast/multicast behavior on every operating system or Docker bridge.
- Discovery is inventory only; it does not authorize direct transport or enrollment.

## Environment and Cleanup

The test binds the product discovery port on loopback, creates temporary node
contexts, sends bounded datagrams, waits up to 3 seconds for processing, and
stops the receiver. Run serially if another local process uses the discovery port.

## Troubleshooting

- Port already in use: stop the conflicting discovery service and rerun; do not change the product port in the test.
- Candidate unexpectedly trusted: inspect the node registry; discovery must not create trust or sessions.
