# HTTP API

**Status:** CI; route inventory is also a drift check.

## Source

- `tests/http_api_e2e.rs`
- Router declarations and route markers: `src/cli/api.rs`

## Run

```bash
cargo test --test http_api_e2e --locked
```

## Proves

- Every route marker matches the router registration, including health/readiness, config, workspace, scripts/tree/search, environments, runs/traces/queue, batteries, secrets, and node management.
- Bearer capability checks return stable `403` errors for unauthorized reads/writes.
- Authorized requests exercise environment mutation, run enqueue/worker/history, script schema/content, and node enrollment/baseline/Cue boundaries.
- Secret schema defaults, run arguments, queue output, and environment responses are redacted.
- HTTP responses use the JSON envelope and stable error codes.

## Does Not Prove

- Route inventory notes are human pointers and do not prove the named test made the request.
- Long-lived deployment, reverse-proxy, TLS termination, or internet exposure are outside this suite.
- Readiness lifecycle belongs to [Node service](node-service.md).

## Environment and Cleanup

`support::HttpServer` starts an isolated child with a temporary workspace, token,
and bounded startup/read operations. Server guards terminate children on drop.
The test suite does not claim Docker resource cleanup.

## Troubleshooting

- `403`: compare the server's declared capability with the route's operation scope.
- Empty search results: the HTTP search route reads the existing FTS index; the test refreshes it through the CLI first.
