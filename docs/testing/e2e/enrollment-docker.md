# Enrollment: Docker

**Status:** Ignored/manual; selected by the Linux transport certification.

## Source

- `tests/docker_enrollment_e2e.rs`
- `scripts/tasks/cert/transport`

## Run

```bash
cargo test --test docker_enrollment_e2e --locked -- --ignored --nocapture docker_manual_enrollment
```

## Proves

- An untrusted candidate probe is blocked.
- Manual enrollment creates a pending request with reciprocal request/code material.
- Pending state survives stopping and restarting both containers and remains blocked before approval.
- Target approval activates one direction; reciprocal approval activates the other.
- Certificates, codes, request binding, and registry snapshots are checked.

## Does Not Prove

- It does not prove signed-bundle enrollment or authority lifecycle.
- It requires a Docker daemon and intentionally runs a full two-container transaction; it is not part of default Rust CI.

## Bounds and Cleanup

The suite uses bounded Compose commands and health polling. Its guard tears down
containers, volumes, and orphans. The aggregate certification also runs the
induced partial-start cleanup test.

## Troubleshooting

- Request remains pending: this is expected until both approvals are explicit.
- Restart loses pending state: inspect copied SQLite snapshots and treat it as a persistence regression.
