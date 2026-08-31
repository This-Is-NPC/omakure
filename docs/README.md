# Documentation

Omakure is a headless automation runner with three supported surfaces: the CLI,
the authenticated HTTP management API, and the machine-owned `node serve`
process. This index separates the product tour from operator guides and
normative protocol contracts so status claims have one clear owner.

## Start here

- [Fleet model](fleet-model.md): Publisher, Conductor, Performer, and the Health,
  Cue, and Baseline planes.
- [CLI and HTTP usage](usage.md): local runs, enrollment, fleet health, Remote
  Cues, and Baselines.
- [Installation](installation.md): release installation, machine services,
  updates, and the exact platform evidence.
- [Deployment](deployment.md): API and node-service topologies, containers,
  volumes, policy, and security.

## Scripts and local automation

- [Create a script](how-to-create-a-script.md): schema-bearing Bash,
  PowerShell, Python, and embedded Lua scripts.
- [Batteries](batteries.md): external script repositories, provenance, local
  installation, and HTTP restrictions.
- [Environments](environments.md): managed environment files and runtime
  injection.
- [Scheduling](scheduling.md): cron scanning and scheduler lifecycle.
- [Workspace](workspace.md): workspace layout and local SQLite state.
- [Script paths](scripts-path.md): workspace resolution and
  `.omakureignore` rules.
- [Recovery](recovery.md): restart, revocation, reset, and identity replacement.

## Integration

- [HTTP API](http-api.md): routes, authentication, policy, limits, and shared
  operations.
- [AI interface](ai-interface.md): stable JSON envelope, agent verbs, queue,
  history, and traces.
- [CLI and HTTP parity](cli-http-parity.md): adapter coverage and deliberately
  local operations.

## Implemented protocol contracts

- [Direct transport and enrollment](internal/direct-transport-contract.md):
  Noise transport, identity binding, enrollment, replay, and revocation.
- [Health Plane](internal/health-plane-contract.md): Profile, Pulse, Signal,
  authorization, privacy classes, retention, and bounds.
- [Remote Cue](internal/remote-cue-contract.md): receiver-owned authorization,
  refusal codes, secret denial, content binding, and at-most-once execution.
- [Baseline delivery](internal/baseline-delivery.md): signed manifest, two
  authorities, atomic installation, drift, and verified rollback.

## Contributors

- [Architecture](internal/architecture.md): stack, source structure, execution
  paths, and boundaries.
- [Implemented requirements](internal/requirements.md): behavior-to-source
  traceability inventory.
- [Development](internal/development.md): build, test, lint, integration checks,
  and `mise` tasks.
- [Deterministic coverage](internal/coverage.md): pinned LLVM reports,
  baseline gate, source inventory, and Codecov patch status.
- [Changed-function complexity](internal/complexity.md): native complexity
  ratchet, exception lifecycle, deterministic evidence, and CI policy.
- [E2E testing](testing/e2e/README.md): end-to-end coverage and its evidence
  boundaries.
- [Release artifacts](internal/release-artifacts.md): binary-only release
  contract.
- [Environment injection specification](internal/env-injection-spec.md):
  precedence and secret non-persistence.

The requirements inventory owns shipped feature status. Operator guides own
workflows. Internal contracts own wire compatibility and quantitative bounds.
When prose disagrees with the implementation, update both the prose and its
source reference in the same change.
