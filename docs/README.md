# Documentation

Omakure is a headless automation runner with three supported surfaces: the CLI,
the authenticated HTTP management API, and the machine-owned `node serve`
process that provides the trusted fleet layer. Choose the first page for your
job, then follow the topic owner for the details.

## New here

- [CLI and local usage](usage.md): **start here** for workspace setup, a first
  script run, queues, environments, scheduling, and Batteries.
- [Installation](installation.md): release installation, machine services,
  updates, and platform evidence.
- [Fleet model](fleet-model.md): conceptual roles and the Health, Cue, and
  Baseline planes.

## Operators

- [Fleet operations manual](fleet-operations.md): **start here** for node
  initialization, discovery, trust, enrollment, fleet health, Remote Cues,
  Baselines, rollback, and lifecycle Signals.
- [Deployment](deployment.md): API and node-service topologies, containers,
  volumes, policy, security, and certification evidence.
- [Batteries](batteries.md): external script repositories, provenance, local
  installation, and HTTP restrictions.
- [Recovery](recovery.md): restart, revocation, reset, and identity replacement.

## Integrators/AI agents

- **Integrators:** start with the [HTTP API](http-api.md) for routes,
  authentication, policy, limits, shared operations, and audit behavior.
- **AI agents:** start with the [AI interface](ai-interface.md) for the stable
  JSON envelope, agent verbs, queue, history, traces, secret handling, and
  storage boundaries.
- [CLI and HTTP parity](cli-http-parity.md): adapter coverage and deliberately
  local operations.
- [Environment injection](environments.md): managed environment files and
  runtime injection.

## Contributors/maintainers

- [Development](internal/development.md): **start here** for build, test, lint,
  integration checks, and `mise` tasks.
- [Architecture](internal/architecture.md): stack, source structure, execution
  paths, and boundaries.
- [Implemented requirements](internal/requirements.md): behavior-to-source
  traceability inventory.
- [Deterministic coverage](internal/coverage.md): pinned LLVM reports,
  repository-owned baseline gate, and source inventory.
- [Changed-function complexity](internal/complexity.md): native complexity
  ratchet, exception lifecycle, deterministic evidence, and CI policy.
- [E2E testing](testing/e2e/README.md): end-to-end coverage and its evidence
  boundaries.
- [Release artifacts](internal/release-artifacts.md): binary-only release
  contract.
- [Environment injection specification](internal/env-injection-spec.md):
  precedence and secret non-persistence.

## Protocol contracts

These documents own wire compatibility, authorization order, privacy classes,
and quantitative bounds:

- [Direct transport and enrollment](internal/direct-transport-contract.md):
  Noise transport, identity binding, enrollment, replay, and revocation.
- [Health Plane](internal/health-plane-contract.md): Profile, Pulse, Signal,
  authorization, privacy classes, retention, and bounds.
- [Remote Cue](internal/remote-cue-contract.md): receiver-owned authorization,
  refusal codes, secret denial, content binding, and at-most-once execution.
- [Baseline delivery](internal/baseline-delivery.md): signed manifest, two
  authorities, atomic installation, drift, and verified rollback.

## Topic guides

- [Create a script](how-to-create-a-script.md): schema-bearing Bash,
  PowerShell, Python, and embedded Lua scripts.
- [Scheduling](scheduling.md): cron scanning and scheduler lifecycle.
- [Workspace](workspace.md): workspace layout and local SQLite state.
- [Script paths](scripts-path.md): workspace resolution and `.omakureignore`
  rules.

## Referência

- [CLI reference](cli-reference.md)
- [Usage Markdown](usage/omakure.md)
- [Usage man page](usage/omakure.1)
- [Usage KDL](usage/omakure.kdl)
- [Operation catalog](operation-catalog.md)
- [Operation support matrix](operation-support-matrix.md)
- [CLI and HTTP parity](cli-http-parity.md)

The requirements inventory owns shipped feature status. Operator guides own
workflows. Internal contracts own wire compatibility and quantitative bounds.
When prose disagrees with the implementation, update both the prose and its
source reference in the same change.
