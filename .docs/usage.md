# CLI and HTTP usage

Omakure has no interactive application mode. Running the binary without a
subcommand prints help; every operational invocation names a command.

## Discovery and workspace

```bash
omakure --help
omakure help-ai
omakure --json config
omakure --json doctor
omakure --json scripts
omakure --scripts-dir /path/to/workspace --json scripts
```

The workspace is selected by `--scripts-dir`, then `OMAKURE_SCRIPTS_DIR`, then
legacy environment overrides, then the debug `scripts/` fixture and platform
defaults. A positional path is not accepted and must not be reintroduced as a
headless alias.

## Scripts and runs

```bash
omakure --json scripts --tag ops --tag production
omakure --json describe tools/deploy.py
omakure --json search deploy --tag production
omakure init tools/job.sh
omakure --json init tools/job.sh --schema-json @schema.json --body-stdin < body.sh
omakure --json run tools/deploy.py --actor agent --reason rollout -- --target prod
omakure --json run tools/deploy.py --env-file ./prod.env --no-prompt -- --target prod
```

Schemas use PascalCase keys and comment markers. See
`how-to-create-a-script.md`. Supported script extensions are `.bash`, `.sh`,
`.ps1`, and `.py`.

## Queue and history

```bash
omakure --json queue add tools/deploy.py --actor agent --priority 10 -- --target prod
omakure queue worker --concurrency 4
omakure --json queue cancel RUN_ID --reason "superseded"
omakure --json queue dead-letter RUN_ID --reason "needs investigation"
omakure --json queue stats
omakure --json history list --state-set all --limit 20
omakure --json history show RUN_ID
omakure --json history stats
omakure --json history traces RUN_ID --since-sequence 0
```

Workers reclaim expired leases. Direct, queued, and scheduled runs share the
same executor, timeout, cancellation, redaction, and SQLite state machine.

## Environments

```bash
omakure env list
omakure env create prod HOST=prod.example.com API_KEY=secret://prod/api_key
omakure env show prod
omakure env set prod REGION=eastus
omakure env replace prod HOST=prod.example.com REGION=eastus
omakure env activate prod
omakure env deactivate
omakure env delete prod
```

Managed files live under `.omakure/envs/`. Active values are injected into
child processes; `--env-file` overrides them for one run and reserved Omakure
variables win last. Sensitive values are masked in diagnostics and excluded
from run storage. See `environments.md` and `env-injection-spec.md`.

## HTTP API and node service

API-only mode:

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --capability all
```

The recommended single-process deployment is:

```bash
omakure token generate --id local --scope '*' --json
omakure node serve --workers 1 --tokens-file /run/secrets/omakure_tokens.toml
```

The default bind is `127.0.0.1:7878`; non-loopback binding requires
`--allow-non-loopback`. Prefer tokens-file auth with per-token scopes. Legacy
`OMAKURE_API_TOKEN` plus `--capability` remains available for local migration.

Unauthenticated probes:

```bash
curl http://127.0.0.1:7878/v1/health
curl http://127.0.0.1:7878/v1/ready
```

All other routes require `Authorization: Bearer <token>`. See `http-api.md`
for routes and `deployment.md` for policy and container operation.

## Fleet health

An enrolled Performer reports a Profile on connect or material change and a
Pulse on a bounded cadence, over the same authenticated direct transport it
already uses. Reporting needs no flag: it starts when `node serve` has a peer
trusted in the `conductor` role with the `inventory-health` capability, and it
stops the moment that trust is revoked or narrowed.

On the Conductor, one bounded view answers "which of my nodes are up":

```bash
omakure node health --json
curl -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/node/health
```

Both render the same operation, so they always agree. Each actively trusted
peer appears once with its presence (`unknown` before it has ever reported,
then `online`, `stale`, or `offline`), its platform and runtime facts, its
runner and scheduler state, and its last completed run.

This is current status only. There is no chart, alert rule, host inventory,
raw log, or history API, and no HTTP call can write health state: the only
writer is the node-to-node exchange. `.docs/health-plane-contract.md` holds the
frozen windows, bounds, error codes, and privacy classes.

## Transport certification

The repository's bounded Linux certification uses four isolated Compose services
and production node listeners. It covers encrypted direct probes, malformed,
oversized, downgraded, expired, forged, identity-mismatched, wrong-target,
untrusted, and exact-replay ingress, durable redacted audits with unchanged
registry-state snapshots, static-peer locator validation, partition/reconnect,
revocation, identity reset/replacement, and the retained discovery/manual/
signed-bundle suites.

```bash
mise run transport-certification
```

This is a development and CI gate, not a general fleet launcher. Nostr, remote
Cues, campaigns, MDM, and Lua are outside its scope; the Health Plane has its
own certification in `tests/health_plane_transport_e2e.rs`.

## Scheduling and local lifecycle

```bash
omakure serve
omakure serve --detach
omakure serve --stop
omakure serve --install
omakure serve --status
omakure serve --uninstall
```

`node serve` enables the scheduler by default; use `--no-scheduler` for API-only
or worker-only test fixtures. See `scheduling.md` for cron and overlap rules.

## Batteries, tokens, and lifecycle

```bash
omakure battery list
omakure battery add https://example.invalid/automation.git --name automation
omakure battery sync automation
omakure battery inspect automation
omakure battery scripts automation
omakure battery install automation tools/job.sh
omakure token generate --id ci --scope runs:enqueue --scope runs:read
omakure completion bash
omakure update --version vX.Y.Z
omakure uninstall
```

Cached Battery repositories are untrusted and never run directly. `update` and
`uninstall` are local lifecycle operations and are not HTTP routes.
