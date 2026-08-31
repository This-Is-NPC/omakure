# CLI and HTTP usage

Omakure has no interactive application mode. Running the binary without a
subcommand prints help; every operational invocation names a command.

This page is the compatibility entry point for local work: select a workspace,
run approved scripts, inspect the local queue and history, configure
environments, schedule work, and install scripts from Batteries. Fleet
operations have one owner in the [fleet operations manual](fleet-operations.md).

## Discovery and workspace

```bash
omakure --help
omakure help-ai
omakure --json config
omakure doctor
omakure --json scripts
omakure --scripts-dir /path/to/workspace --json scripts
```

The workspace is selected by `--scripts-dir`, then `OMAKURE_SCRIPTS_DIR`, then
legacy environment overrides, then the debug `scripts/workspace` fixture and
platform defaults. A positional path is not accepted and must not be
reintroduced as a headless alias.

`check` is the visible alias for the same workspace diagnostics:

```bash
omakure check
```

Use the [fleet operations manual](fleet-operations.md) for node initialization,
trust-neutral LAN discovery, direct transport probes, peer trust, capabilities,
and enrollment. The [generated CLI reference](cli-reference.md) has the complete
flag metadata.

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
[Create a script](how-to-create-a-script.md). Supported script extensions are
`.bash`, `.sh`, `.ps1`, `.py`, and `.lua`. Bash, PowerShell, and Python need
their interpreter installed; `.lua` does not, because the Lua runtime is
embedded in the binary. Extensionless names resolve in that order, so
`omakure run deploy` picks `deploy.sh` over `deploy.lua`.

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

To supply a caller-owned correlation id, pass `--run-id`:

```bash
omakure --json queue add tools/deploy.py --run-id deploy-2026-08-30
```

The id is stored with the queued row and appears in worker/history output.
Workers reclaim expired leases. Direct, queued, and scheduled runs share the
same executor, timeout, cancellation, redaction, and SQLite state machine.
See the [AI interface](ai-interface.md) for the machine-facing queue, history,
and trace contract.

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
from run storage. See [Environments](environments.md) and the
[environment injection specification](internal/env-injection-spec.md).

## HTTP API and node service

Use these commands to start the local management surfaces. Route semantics,
authentication, scopes, request limits, and response envelopes belong to the
[HTTP API guide](http-api.md); bind policy, topology, workers, containers, and
certification belong to [Deployment](deployment.md).

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

## Scheduling

`node serve` enables the scheduler by default; use `--no-scheduler` for API-only
or worker-only test fixtures. Scheduler and workers must use one host-local
workspace and its `.history/runs.sqlite`; this is not a cross-host queue. See
[Scheduling](scheduling.md) for cron and overlap rules.

## Local service lifecycle

```bash
omakure serve
omakure serve --detach
omakure serve --stop
omakure serve --install
omakure serve --status
omakure serve --uninstall
```

These commands manage the local scheduler service. The `node serve` process
also owns the authenticated HTTP API and machine-node sessions; see the
[HTTP API guide](http-api.md) and [Deployment](deployment.md).

## Batteries

```bash
omakure battery list
omakure battery add https://example.invalid/automation.git --name automation
omakure battery sync automation
omakure battery inspect automation
omakure battery scripts automation
omakure battery install automation tools/job.sh
```

For private HTTPS repositories, keep the credential out of the registry and
store only a provider reference with `--token-ref`:

```bash
omakure battery add https://example.invalid/private.git --name private \
  --token-ref secret://git/token
```

The configured secret provider must resolve the reference at sync time; a
missing or unauthorized reference fails before the repository is fetched.

Cached Battery repositories are untrusted and never run directly. Battery
installation is Unix-only and is a local act that an authenticated HTTP
operator may initiate, but a peer or Remote Cue never may. See [Batteries](batteries.md)
for repository validation, provenance, and install rules.

## Tokens

```bash
omakure token generate --id ci --scope runs:enqueue --scope runs:read
```

Use the generated token with the preferred tokens-file authentication described
in the [HTTP API authentication contract](http-api.md#authentication-contract).

## Local lifecycle

```bash
omakure update --version vX.Y.Z
omakure uninstall
```

`update` and `uninstall` are local lifecycle operations and are not HTTP
routes. See [Installation](installation.md) for release lifecycle details.

## `omakure completion`

**Synopsis:** `omakure completion SHELL`

Completion generation requires only the installed binary and a supported shell.
It writes a completion script to stdout and does not mutate workspace state.
Success is shell syntax suitable for the shell's completion directory. The
principal failure is an unsupported `SHELL` value; no file is written by the
command itself.

```bash
omakure completion bash >> ~/.bash_completion
```

`SHELL` is one of `bash`, `zsh`, `fish`, or `pwsh`; the positional value is
required. See the [generated CLI reference](cli-reference.md#omakure-completion).

For fleet status, Remote Cues, signed Baselines, rollback, and lifecycle
Signals, use the [fleet operations manual](fleet-operations.md). Its commands
are local operator actions or node-to-node workflows; no local quickstart here
repeats their contracts.
