# omakure

Omakure is a headless Rust automation runner. The product surface is a
machine-readable CLI plus an authenticated HTTP API. `omakure node serve` is
the machine-owned service that combines the API, optional queue workers, and
the schedule scanner into one deployable process.

Scripts are ordinary Bash, PowerShell, or Python files with an embedded JSON
schema between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`. Runs, queue
state, and structured traces are recorded in SQLite under the workspace.

## Requirements

- Rust toolchain for development
- Git, Bash, and `jq` for the required runtime checks
- Optional PowerShell (`pwsh`) for `.ps1` scripts
- Optional Python 3 for `.py` scripts
- Nothing for `.lua` scripts: the Lua runtime is embedded in the binary

The default workspace is `~/Documents/omakure-scripts` on Linux/macOS and
`%USERPROFILE%\Documents\omakure-scripts` on Windows. Set
`OMAKURE_SCRIPTS_DIR` or pass `--scripts-dir <PATH>` to select another one.

## Quick start

Install a release:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.sh \
  | bash -s -- --repo This-Is-NPC/omakure
```

Discover the compiled CLI surface and inspect the workspace:

```bash
omakure --help
omakure help-ai
omakure doctor
omakure --json scripts
```

Start a local node service. This example uses the legacy local token mode;
deployments should prefer a `--tokens-file` with scoped Argon2id tokens.

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure node serve --workers 1 --no-scheduler \
  --capability all >./omakure-node.log 2>&1 &
NODE_PID=$!
trap 'kill "$NODE_PID" 2>/dev/null || true' EXIT

curl -fsS http://127.0.0.1:7878/v1/health
curl -fsS http://127.0.0.1:7878/v1/ready
curl -fsS -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/scripts
```

Create and run a representative script through the CLI:

```bash
omakure --json init hello.sh \
  --schema-json '{"Name":"hello","Fields":[]}' \
  --body-stdin <<'BODY'
#!/usr/bin/env bash
printf 'hello from omakure\n'
BODY
omakure --json run hello.sh --actor local --reason smoke-test
```

The CLI and HTTP API use the same operations. `--json` emits the stable
`{ ok, data, error, schema_version }` envelope. Use `omakure config` to verify
resolved paths and `omakure doctor` to validate runtimes and schemas.

## Common workflows

```bash
# Catalogue and inspect scripts
omakure --json scripts --tag ops
omakure --json describe tools/deploy.py
omakure --json search deploy --tag production

# Run directly or queue for a worker
omakure --json run tools/deploy.py --actor agent -- --target prod
omakure --json queue add tools/deploy.py --actor agent -- --target prod
omakure queue worker --concurrency 4

# Inspect runs and traces
omakure --json history list --state-set all --limit 20
omakure --json history stats
omakure --json history traces RUN_ID
```

For schedules, use `omakure serve` or enable the scheduler in `omakure node serve`.
For a single deploy unit, use `node serve`; it exposes `/v1/health` and
`/v1/ready` without authentication and protects all other routes with bearer
auth.

## Node Registry Foundation

The portable node foundation owns machine-state `node.sqlite` separately from the
workspace `.history/runs.sqlite`. The headless `node init`, `node status`,
`node peers`, `node trust`, `node capabilities`, and `node revoke` commands,
plus the authenticated `/v1/node/*` management routes, use shared operations.
Public output contains only the x-only identity and redacted/bounded state.
Trust mutations require explicit confirmation, actor, and reason evidence.
`node serve` is the completed portable node lifecycle foundation. It creates
one machine identity and an empty trust registry on first start, preserves
them across restarts, workspaces, updates, and uninstall, and requires
`omakure node reset --confirmed` for destructive removal. Peer discovery,
direct transport, LAN discovery, manual enrollment, and signed-bundle enrollment
are implemented and covered by focused protocol tests plus the bounded Linux
certification gate. Nostr, Pulses, Profiles, Signals, remote Cues, campaigns,
MDM, and Lua remain explicitly deferred. Installed-service ownership and ACLs
are platform-specific release gates; this Linux development run does not
fabricate macOS or Windows runtime evidence.

## Documentation

- `rebuild-omakure.md`: canonical future product direction and node contract
- `docs/internal/direct-transport-contract.md`: implemented direct transport/enrollment wire and state contract
- `docs/README.md`: documentation index
- `docs/usage.md`: CLI and HTTP workflows
- `docs/ai-interface.md`: JSON, queue, history, trace, and agent contract
- `docs/http-api.md`: routes, authentication, policy, and parity
- `docs/deployment.md`: node-service/container deployment and security
- `docs/recovery.md`: restart, revocation, reset, and identity-replacement recovery
- `docs/workspace.md`: on-disk state and ownership
- `docs/scripts-path.md`: workspace resolution and ignore files
- `docs/how-to-create-a-script.md`: schema and script authoring
- `docs/scheduling.md`: cron scheduler lifecycle
- `docs/headless-migration.md`: breaking removals and migration actions
- `docs/headless-release.md`: current headless release contract
- `docs/internal/development.md`: local build, test, lint, and node-service checks
- `docs/internal/architecture.md`: source structure and retained dependencies
- `docs/internal/requirements.md`: implemented requirements with source references

Historical release notes retain the behavior of the releases that produced
them. The current headless breaking changes are documented separately in the
migration and release documents above.

## License

Apache-2.0. See `LICENSE`.
