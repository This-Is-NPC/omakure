# Secret Redaction

**Status:** CI.

## Source

- `tests/secret_cli_e2e.rs`
- Secret-related cases in `tests/http_api_e2e.rs` and `tests/cli_battery.rs`
- Shared assertions in `tests/support/mod.rs`

## Run

```bash
cargo test --test secret_cli_e2e --test http_api_e2e --test cli_battery --locked
```

## Proves

- Secret fields and secret references are absent from stdout, stderr, JSON, run history, traces, queue rows, and HTTP responses.
- Authorized `secret://` references resolve only when the capability and declared reference permit them.
- Plaintext secret fields and secret arguments are rejected rather than persisted in unreconstructable form.
- Environment display masks sensitive values, while ordinary values remain usable.
- Queue workers execute secret-backed scripts without exposing the resolved value.
- HTTP schema secret defaults become `null` rather than returning plaintext.

## Does Not Prove

- It does not prove that an external secret manager is secure or that arbitrary child-process output is safe outside Omakure's redaction paths.
- It does not inspect production logs from a deployed logging stack.

## Environment and Cleanup

Fixtures use deliberately recognizable values. They are asserted absent from
all captured output. Temporary workspaces and queue databases are removed by
the harness; never copy fixture values into bug reports.

## Troubleshooting

- A redaction assertion fails: preserve only the test name and safe envelope/error code; remove the captured value before sharing output.
- A reference is forbidden: compare the exact configured `--secret-ref` and `secrets:use` capability.
