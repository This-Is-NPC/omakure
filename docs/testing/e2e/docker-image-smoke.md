# Docker Image Smoke

**Status:** CI, Linux `docker-smoke` job.

## Source

- `.github/workflows/ci.yml`, job `docker-smoke`
- `Dockerfile`, `tests/packaging_smoke.rs`

## Run

```bash
timeout --foreground --kill-after=10s 10m docker build --tag omakure-node:ci .
```

The workflow then starts the image with workspace and `/var/lib/omakure`
volumes, publishes `127.0.0.1:7878`, and polls `/v1/health` and `/v1/ready`.

## Proves

- The packaged image builds and starts as the fixed non-root service account.
- Volume ownership setup (`10001:10001`, workspace `0750`, state `0700`) permits startup.
- Health and readiness work through the image's default node service.
- The image has no system `lua`, yet the embedded Lua host executes a probe and reports Lua 5.4.
- Two packaged containers carry one authorized Cue over their standing service session.
- Two packaged containers plus a publisher carry a signed baseline, detect drift, push a new version, and roll back; revoked-publisher rollback is refused.
- Always-run cleanup verifies containers, networks, and named volumes are gone.

## Does Not Prove

- It is not a general Docker deployment or multi-host network test.
- It does not execute the standalone ignored Docker discovery/enrollment Rust suites.
- It does not prove installer registration with a host service manager.

## Bounds and Cleanup

Image build is bounded at 10 minutes. Health/readiness attempts use 30 attempts
with bounded curl calls. Cue and baseline operations use bounded Docker calls and
polling. Cleanup removes all named image-smoke resources and fails if inspection
cannot run or anything remains.

## Troubleshooting

- `lua` unexpectedly exists: the self-containment proof intentionally fails because the image changed.
- Readiness fails: inspect `docker logs omakure-node-ci` before cleanup removes the container.
- Cue path reports the wrong `via`: the standing session did not establish, so the intended service path was not tested.
