# Documentation index

The repository is a headless automation product. Its supported integration
surfaces are the CLI, the HTTP management API, and the machine-owned `node serve`
process.

## Users and operators

- `../rebuild-omakure.md`: canonical future product direction and node contract
- `direct-transport-contract.md`: implemented direct transport/enrollment wire and state contract
- `health-plane-contract.md`: frozen Profile/Pulse/Signal wire, authorization, and bounds contract (pending owner review)
- `remote-cue-contract.md`: frozen Cue wire, authorization gates, and refusal codes
- `baseline-delivery.md`: frozen signed-baseline manifest, carriage, install, and rollback contract
- `installation.md`: install, update, uninstall, and version pinning.
- `usage.md`: CLI commands and common workflows.
- `deployment.md`: API/node-service topologies, containers, volumes, and security.
- `recovery.md`: restart, revocation, reset, and identity-replacement recovery.
- `http-api.md`: HTTP routes, auth, policy, limits, and shared operations.
- `scheduling.md`: cron scheduler lifecycle and systemd autostart.
- `workspace.md`: workspace layout and SQLite runtime state.
- `scripts-path.md`: workspace resolution and `.omakureignore` rules.
- `environments.md`: managed environment files and runtime injection.
- `batteries.md`: reusable script repositories and provenance.
- `how-to-create-a-script.md`: schema and script authoring.

## AI and integration

- `ai-interface.md`: JSON envelope, agent verbs, queue, history, and traces.
- `cli-http-parity.md`: CLI, shared operation, and HTTP parity matrix.
- `headless-migration.md`: intentional breaking removals and migration actions.

## Contributors

- `development.md`: build, test, lint, integration checks, and `mise` tasks.
- `architecture.md`: retained stack, source structure, and boundaries.
- `requirements.md`: implemented requirements with source references.
- `release-artifacts.md`: binary-only release archive contract.
- `headless-release.md`: current release checklist and compatibility statement.
- `env-injection-spec.md`: environment precedence and secret non-persistence.
