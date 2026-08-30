# End-to-End Testing

This directory is the source-of-truth map for Omakure's black-box, integration,
Docker, and certification coverage. It documents the repository as it exists;
it does not turn an ignored or manual test into a CI guarantee.

## Status

- **CI**: invoked by a normal CI or release workflow on at least one stated platform.
- **Manual**: requires a local Docker, VM, network, or host-service prerequisite.
- **Ignored**: marked `#[ignore]`; it must be selected explicitly.
- **Certification**: a bounded gate with explicit cleanup and post-run resource inspection.

## Guides

- [CLI surface](cli-surface.md)
- [HTTP API](http-api.md)
- [Node service](node-service.md)
- [Deploy policy](deploy-policy.md)
- [Package and release](package-and-release.md)
- [Docker image smoke](docker-image-smoke.md)
- [Secret redaction](secret-redaction.md)
- [Script runtime](script-runtime.md)
- [Test harness](test-harness.md)

## Native Protocol Coverage

- [Direct transport](direct-transport-native.md)
- [Discovery](discovery-native.md)
- [Enrollment](enrollment-native.md)
- [Remote Cue](remote-cue-native.md)
- [Baseline](baseline-native.md)
- [Health Plane](health-plane-native.md)

## Docker and Host Certification

- [Direct transport Docker](direct-transport-docker.md)
- [Discovery Docker](discovery-docker.md)
- [Enrollment Docker](enrollment-docker.md)
- [Signed bundle Docker](signed-bundle-docker.md)
- [Health Plane Docker](health-plane-docker.md)
- [Transport certification](transport-certification.md)
- [Fedora VM privilege](fedora-vm-privilege.md)

## Common Rules

- Use `--scripts-dir` or the documented workspace environment; never assume the current directory is a workspace.
- Use `cargo test --locked` for reproducible Rust runs.
- Docker certification Compose files are ephemeral topologies, not deployment examples.
- Tests use temporary workspaces, volumes, and state. A successful test is not cleanup evidence unless the harness checks cleanup.
- Never paste bearer tokens, token hashes, bootstrap tokens, private keys, or secret fixture values into failure reports.

See [`_template.md`](_template.md) when adding a new guide.
