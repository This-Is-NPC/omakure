# Test Harness

**Status:** CI self-tests plus shared infrastructure for all suites.

## Source

- `tests/support_harness.rs`
- `tests/support/mod.rs`

## Run

```bash
cargo test --test support_harness --locked
```

## Proves

- Temporary workspaces, schema scripts, JSON envelopes, executable fixtures, and secret assertions are available and isolated.
- Child commands have bounded execution and useful captured output.
- HTTP helpers allocate loopback ports, wait for readiness, read responses across timeouts, and fail when a peer never closes.
- Teardown helpers terminate children and remove temporary state.

## Does Not Prove

- Harness self-tests do not prove the product behavior of every suite that uses the helpers.
- A successful drop/teardown path is not Docker or libvirt cleanup evidence.

## Environment and Cleanup

The harness owns `TempDir` workspaces and process guards. Keep tests independent:

## Troubleshooting

- Flaky port allocation: use `unique_loopback_port()` and retry through the helper rather than hard-coding a port.
- Truncated response: distinguish a peer that sends delayed chunks from one that never closes; the harness has separate tests for both.
