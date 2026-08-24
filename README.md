# omakure

Omakure is a headless Rust automation runner. The product surface is a
machine-readable CLI plus an authenticated HTTP API. `omakure engine` combines
the API, optional queue workers, and the schedule scanner into one deployable
process.

Scripts are ordinary Bash, PowerShell, or Python files with an embedded JSON
schema between `OMAKURE_SCHEMA_START` and `OMAKURE_SCHEMA_END`. Runs, queue
state, and structured traces are recorded in SQLite under the workspace.

## Requirements

- Rust toolchain for development
- Git, Bash, and `jq` for the required runtime checks
- Optional PowerShell (`pwsh`) for `.ps1` scripts
- Optional Python 3 for `.py` scripts

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

Start a local headless engine. This example uses the legacy local token mode;
deployments should prefer a `--tokens-file` with scoped Argon2id tokens.

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure engine --workers 1 --no-scheduler \
  --capability all >./omakure-engine.log 2>&1 &
ENGINE_PID=$!
trap 'kill "$ENGINE_PID" 2>/dev/null || true' EXIT

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

For schedules, use `omakure serve` or enable the scheduler in `omakure engine`.
For a single deploy unit, prefer `engine`; it exposes `/v1/health` and
`/v1/ready` without authentication and protects all other routes with bearer
auth.

## Node Registry Foundation

The node foundation owns machine-state `node.sqlite` separately from the
workspace `.history/runs.sqlite`. The headless `node init`, `node status`,
`node peers`, `node trust`, `node capabilities`, and `node revoke` commands,
plus the authenticated `/v1/node/*` management routes, use shared operations.
Public output contains only the x-only identity and redacted/bounded state.
Trust mutations require explicit confirmation, actor, and reason evidence.
Peer discovery, enrollment, transport, Cue execution, and `node serve` are
intentionally not part of this package surface. The registry uses the same
source-level contract on Linux, macOS, and Windows, while installed-service
ownership, ACLs, and cross-target release validation remain platform-specific
release gates and are not simulated by the Linux development test run.

## Documentation

- `rebuild-omakure.md`: canonical future product direction and node contract
- `.docs/README.md`: documentation index
- `.docs/usage.md`: CLI and HTTP workflows
- `.docs/ai-interface.md`: JSON, queue, history, trace, and agent contract
- `.docs/http-api.md`: routes, authentication, policy, and parity
- `.docs/deployment.md`: engine/container deployment and security
- `.docs/workspace.md`: on-disk state and ownership
- `.docs/scripts-path.md`: workspace resolution and ignore files
- `.docs/how-to-create-a-script.md`: schema and script authoring
- `.docs/scheduling.md`: cron scheduler lifecycle
- `.docs/headless-migration.md`: breaking removals and migration actions
- `.docs/headless-release.md`: current headless release contract
- `.docs/development.md`: local build, test, lint, and engine smoke checks
- `.docs/architecture.md`: source structure and retained dependencies
- `.docs/requirements.md`: implemented requirements with source references

Historical release notes retain the behavior of the releases that produced
them. The current headless breaking changes are documented separately in the
migration and release documents above.

## License

Apache-2.0. See `LICENSE`.
