# Direct Transport: Native

**Status:** CI for contract and ordinary native cases; selected multi-process cases are ignored/manual.

## Source

- `tests/direct_transport_contract.rs`
- `tests/direct_transport_e2e.rs`
- `src/direct_transport.rs`

## Run

```bash
cargo test --test direct_transport_contract --locked
cargo test --test direct_transport_e2e --locked
cargo test --test direct_transport_e2e --locked -- --ignored --nocapture
```

The normal CI matrix explicitly names the contract suite. The Linux transport
certification additionally runs `direct_transport_e2e` with a five-minute bound.

## Proves

- Frozen Noise/frame/probe vectors and production handshake behavior remain compatible.
- Trusted peers can probe over encrypted direct TCP, receive acknowledgements, and record accepted audits.
- Wrong identity, expired/forged/mismatched certificates, forged envelopes, replays, wrong targets, and post-reset old identities are rejected without unauthorized registry mutation.
- Disconnect/reconnect creates a new session and preserves audit evidence.

## Does Not Prove

- Native tests do not prove Docker networking or image contents; see [Direct transport Docker](direct-transport-docker.md).
- Ignored tests are not a default `cargo test` guarantee.

## Bounds and Cleanup

Native process probes use bounded waits (normally 12 seconds for connection,
5 seconds for audit/rejection observation, and 2 seconds for raw socket I/O).
Temporary node state and child services are dropped after each case.

## Troubleshooting

- No connection: verify both peers were trusted and static-peer configuration uses the node ID generated in that run.
- Wrong rejection code: inspect the latest `transport_audit` row; the registry snapshot should remain unchanged.
