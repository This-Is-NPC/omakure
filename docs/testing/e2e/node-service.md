# Node Service

**Status:** CI on all supported runners, with Unix SIGTERM coverage and platform-specific process checks.

## Source

- `tests/node_service_e2e.rs`
- `tests/support/mod.rs`

## Run

```bash
cargo test --test node_service_e2e --test policy_e2e --locked
mise run dev
```

## Proves

- A service with zero workers and no scheduler serves health/readiness and leaves an enqueued run queued.
- A service with one worker completes an enqueued script and exposes stdout.
- Readiness is unauthenticated, minimal, and can require worker/scheduler flags.
- Identity and node registry state are created once and survive restart.
- Corrupt or missing registry state fails before readiness without replacing identity.
- Active-service `init` and confirmed `reset` conflict safely; reset is workspace-independent and changes identity only after the service is stopped.
- Unix SIGTERM and portable terminate/restart paths exit cleanly.

## Does Not Prove

- It does not prove systemd, launchd, or Windows Service registration.
- Scheduler job semantics are not covered by the no-scheduler lifecycle tests.
- Container ownership and image startup belong to [Docker image smoke](docker-image-smoke.md).

## Environment and Cleanup

Startup is bounded at 15 seconds, readiness polling at 10 seconds, worker
completion at 20 seconds, and lifecycle termination at 15 seconds. Temporary
workspaces and child processes are cleaned by the shared harness.

## Troubleshooting

- No readiness: inspect the child stderr for corrupt state, invalid config, or token recovery errors; do not recreate identity by hand.
- Run remains queued: confirm the test intentionally used `--workers 0`.
