<!-- BEGIN GENERATED CLI REFERENCE -->
# CLI reference

This section is generated from Clap metadata. Run `cargo run --bin cli-reference` to refresh it.

## `omakure api`

Run the internal HTTP management API

- **ID:** `api`
- **Visibility:** visible

### Options

- `--allow-non-loopback` — Explicitly allow the HTTP API to bind to non-loopback addresses (values: `false`, `true`)
- `--bind BIND` — Address to bind the HTTP API server to (default: `127.0.0.1:7878`)
- `--capability CAPABILITIES` — API capability to grant in legacy single-token mode (`OMAKURE_API_TOKEN`). Repeatable. Ignored when `--tokens-file` is set. Supported: config:read, scripts:read, env:read / envs:read, env:write / envs:write, env:activate / envs:activate, env:use / envs:use, secrets:use, secrets:read-metadata, credentials:use, runs:read, runs:write / runs:enqueue, batteries:read, batteries:write, admin:status, all. Node management uses narrow node:read, node:write, and trust:write capabilities. `all` grants every route capability but does not bypass `--secret-ref` (pass `--secret-ref '*'` for unrestricted refs)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--policy POLICY` — Deploy-only policy.toml (route groups + auth/node-service defaults). Overrides `OMAKURE_POLICY_FILE`. Separate from workspace omakure.toml
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--secret-ref SECRET_REFS` — Allowed secret provider ref for secrets:use / credentials:use, e.g. secret://prod/token or secret://prod/*; repeatable. Empty denies provider refs
- `--tokens-file TOKENS_FILE` — Multi-token TOML file (Argon2id hashes + per-token scopes). Overrides `OMAKURE_TOKENS_FILE`. When set, process-wide `--capability` is ignored; scopes come from each token

## `omakure battery`

Manage reusable Battery automation repositories

- **ID:** `battery`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`battery add`](#omakure-battery-add)
- [`battery inspect`](#omakure-battery-inspect)
- [`battery install`](#omakure-battery-install)
- [`battery list`](#omakure-battery-list)
- [`battery remove`](#omakure-battery-remove)
- [`battery scripts`](#omakure-battery-scripts)
- [`battery sync`](#omakure-battery-sync)

## `omakure battery add`

Register a Battery repository source

- **ID:** `battery add`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--name NAME` — Stable Battery name (lowercase kebab-case) **(required)**
- `--ref REQUESTED_REF` — Branch, tag, or ref to sync (default: `main`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--token-ref TOKEN_REF` — Secret ref for private HTTPS auth (`secret://provider/key`). Registry stores the ref only; sync resolves via GIT_ASKPASS
- `GIT_URL` — Git repository URL or local path **(required)**

## `omakure battery inspect`

Inspect one synced Battery manifest

- **ID:** `battery inspect`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` — Battery name **(required)**

## `omakure battery install`

Install one Battery script into the trusted scripts workspace

- **ID:** `battery install`
- **Visibility:** visible

### Options

- `--force` — Overwrite an existing script target (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` — Battery name **(required)**
- `SCRIPT_ID` — Script id from `omakure battery scripts <name>` **(required)**

## `omakure battery list`

List registered Batteries

- **ID:** `battery list`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure battery remove`

Unregister one Battery

- **ID:** `battery remove`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--remove-cache` — Also delete the cached clone (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` — Battery name **(required)**

## `omakure battery scripts`

List installable scripts from one Battery

- **ID:** `battery scripts`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` — Battery name **(required)**

## `omakure battery sync`

Fetch and validate a Battery checkout

- **ID:** `battery sync`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` — Battery name **(required)**

## `omakure completion`

Generate shell completion script for the given shell

- **ID:** `completion`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `SHELL` — Shell to generate completions for **(required)** (values: `bash`, `fish`, `pwsh`, `zsh`)

## `omakure config`

Show resolved paths and environment diagnostics

- **ID:** `config`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure describe`

Show the full schema of one script

- **ID:** `describe`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `SCRIPT` — Script name or path **(required)**

## `omakure doctor`

Check runtime dependencies and workspace

- **ID:** `doctor`
- **Aliases:** `check`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure env`

Manage named environment files

- **ID:** `env`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`env activate`](#omakure-env-activate)
- [`env create`](#omakure-env-create)
- [`env deactivate`](#omakure-env-deactivate)
- [`env delete`](#omakure-env-delete)
- [`env list`](#omakure-env-list)
- [`env remove`](#omakure-env-remove)
- [`env replace`](#omakure-env-replace)
- [`env set`](#omakure-env-set)
- [`env show`](#omakure-env-show)

## `omakure env activate`

Activate a named environment

- **ID:** `env activate`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` **(required)**

## `omakure env create`

Create a named environment from optional `KEY=value` pairs

- **ID:** `env create`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` **(required)**
- `KEY=VALUE`

## `omakure env deactivate`

Deactivate the current environment

- **ID:** `env deactivate`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure env delete`

Delete a named environment

- **ID:** `env delete`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` **(required)**

## `omakure env list`

List named environments

- **ID:** `env list`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure env remove`

Remove one key from a named environment

- **ID:** `env remove`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `KEY` **(required)**
- `NAME` **(required)**

## `omakure env replace`

Replace a named environment with the provided `KEY=value` pairs

- **ID:** `env replace`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` **(required)**
- `KEY=VALUE`

## `omakure env set`

Set one `KEY=value` in a named environment

- **ID:** `env set`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` **(required)**
- `KEY=VALUE` **(required)**

## `omakure env show`

Show a named environment with sensitive values redacted

- **ID:** `env show`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NAME` **(required)**

## `omakure help-ai`

Print the AI capability surface as JSON

- **ID:** `help-ai`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure history`

Query the run history

- **ID:** `history`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`history list`](#omakure-history-list)
- [`history show`](#omakure-history-show)
- [`history stats`](#omakure-history-stats)
- [`history tail`](#omakure-history-tail)
- [`history traces`](#omakure-history-traces)

## `omakure history list`

List recent runs

- **ID:** `history list`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Filter by actor tag (e.g. `human`, `ai`)
- `--failure` — Only failed runs (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--limit LIMIT` — Maximum number of rows to return
- `--script SCRIPT` — Filter by script name or path substring
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--since SINCE` — Only runs since this duration ago (e.g. `1d`, `30m`, `12h`)
- `--state STATE` — Filter by run state (repeatable; logical OR within the flag). Valid values: queued, running, completed, failed, cancelled, timed_out, dead_letter. Mutually exclusive with `--state-set`
- `--state-set STATE_SET` — Filter by a named state group: `in_flight` (queued+running), `terminal` (everything else), or `all`. Default when neither `--state` nor `--state-set` is set: `terminal` so existing callers see no behavior change
- `--success` — Only successful runs (values: `false`, `true`)
- `--until UNTIL` — Only runs until this duration ago

## `omakure history show`

Show one run by id

- **ID:** `history show`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `RUN_ID` — Run id (as printed by `omakure run --json` or `omakure history list`) **(required)**

## `omakure history stats`

Aggregate counts per state and per actor

- **ID:** `history stats`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure history tail`

Print the most recent N runs (no --follow in v1)

- **ID:** `history tail`
- **Visibility:** visible

### Options

- `--follow` — Unsupported; rejected with error.code = "not_implemented" (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--limit LIMIT` — Number of rows to print (default: 10) (default: `10`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure history traces`

Read the structured trace stream of one run

- **ID:** `history traces`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--level LEVEL` — Minimum level (debug, info, warn, error). Defaults to `debug` (returns every record)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--since-sequence SINCE_SEQUENCE` — Return only entries with `sequence > N`. Used by agents for incremental fetches
- `RUN_ID` — Run id **(required)**

## `omakure init`

Create a new script template

- **ID:** `init`
- **Visibility:** visible

### Options

- `--body-stdin` — Read the script body from stdin and write it verbatim under the schema header when `--schema-json` is set. Without `--schema-json`, stdin is ignored and the default placeholder template is written (values: `false`, `true`)
- `--force` — Overwrite an existing script of the same name (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--name SCRIPT` — Script path (legacy)
- `--schema-json SCHEMA_JSON` — Inline schema JSON or `@path/to/schema.json`. When set, the new script is generated with this schema embedded between the `OMAKURE_SCHEMA_START` / `OMAKURE_SCHEMA_END` markers instead of the default placeholder template
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `SCRIPT` — Script path

## `omakure node`

Inspect and explicitly manage the machine-owned node identity and trust registry

- **ID:** `node`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--node-config CONFIG_PATH` — Deterministic test-only node configuration path override
- `--node-state-dir STATE_DIR` — Deterministic test-only node state directory override
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`node authority`](#omakure-node-authority)
- [`node baseline`](#omakure-node-baseline)
- [`node capabilities`](#omakure-node-capabilities)
- [`node cue`](#omakure-node-cue)
- [`node direct-probe`](#omakure-node-direct-probe)
- [`node discovery`](#omakure-node-discovery)
- [`node enroll`](#omakure-node-enroll)
- [`node health`](#omakure-node-health)
- [`node init`](#omakure-node-init)
- [`node peers`](#omakure-node-peers)
- [`node reset`](#omakure-node-reset)
- [`node revoke`](#omakure-node-revoke)
- [`node serve`](#omakure-node-serve)
- [`node signals`](#omakure-node-signals)
- [`node status`](#omakure-node-status)
- [`node trust`](#omakure-node-trust)

## `omakure node authority`

Hold and use this node's enrollment authority

- **ID:** `node authority`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`node authority create`](#omakure-node-authority-create)
- [`node authority issue`](#omakure-node-authority-issue)
- [`node authority show`](#omakure-node-authority-show)

## `omakure node authority create`

Create this node's enrollment authority key, refusing to replace one

- **ID:** `node authority create`
- **Visibility:** visible

### Options

- `--confirmed` — Required. Creating an authority is a fleet-wide act (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node authority issue`

Mint one enrollment bundle naming this node as the subject

- **ID:** `node authority issue`
- **Visibility:** visible

### Options

- `--audience AUDIENCE` — The node that will apply this bundle. It is checked against that node's own identity when it does, so a bundle is useless anywhere else **(required)**
- `--capability CAPABILITIES` — A capability the audience will grant this node. Repeatable
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--lifetime-seconds LIFETIME_SECONDS` — How long the bundle stays valid, in seconds (default: `3600`)
- `--role ROLE` — The role the audience will record for this node **(required)** (values: `conductor`, `performer`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node authority show`

Report the authority this node holds, without its private half

- **ID:** `node authority show`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node baseline`

Publish and deliver the signed set of scripts a fleet runs

- **ID:** `node baseline`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`node baseline create-key`](#omakure-node-baseline-create-key)
- [`node baseline publish`](#omakure-node-baseline-publish)
- [`node baseline push`](#omakure-node-baseline-push)
- [`node baseline rollback`](#omakure-node-baseline-rollback)

## `omakure node baseline create-key`

Create this node's baseline publisher key, refusing to replace one

- **ID:** `node baseline create-key`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node baseline publish`

Sign the named workspace scripts as one baseline

- **ID:** `node baseline publish`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--lifetime-seconds LIFETIME_SECONDS` — How long the baseline stays installable, in seconds (default: `3600`)
- `--out PATH` — Where to write the signed manifest **(required)**
- `--script PATH` — A workspace-relative script path to include. Repeatable
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node baseline push`

Deliver a signed baseline to one trusted Performer

- **ID:** `node baseline push`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--manifest PATH` — The signed manifest produced by `node baseline publish` **(required)**
- `--peer-node-id PEER_NODE_ID` — Expected canonical peer node ID **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--wait-seconds WAIT_SECONDS` — How long to wait on the session for the Performer's answer (default: `120`)

## `omakure node baseline rollback`

Put this node back on the one baseline retained before the current one

- **ID:** `node baseline rollback`
- **Visibility:** visible

### Options

- `--confirmed` — Required. A rollback replaces every script the current baseline named (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node capabilities`

Update one peer's capability allow-list with confirmation and evidence

- **ID:** `node capabilities`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Audit actor **(required)**
- `--capability CAPABILITIES` — Allowed capability (repeatable; sorted unique values are required)
- `--confirmed` — Confirm this trust mutation explicitly (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--reason REASON` — Audit reason/evidence **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NODE_ID` — Peer node identifier **(required)**

## `omakure node cue`

Ask one trusted Performer to run a script it has already declared

- **ID:** `node cue`
- **Visibility:** visible

### Options

- `--direct` — Dial the peer from this process instead of asking the running service (values: `false`, `true`)
- `--endpoint ENDPOINT` — Peer direct transport address **(required)**
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--peer-node-id PEER_NODE_ID` — Expected canonical peer node ID **(required)**
- `--reason REASON` — Why this is being asked for. Recorded in the Performer's audit trail **(required)**
- `--script SCRIPT` — Script name as the Performer declared it. A path is not accepted: the Performer resolves the name against what it published, and a Cue never carries a location **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--wait-seconds WAIT_SECONDS` — How long to stay on the session waiting for the `run-completed` Signal (default: `120`)

## `omakure node direct-probe`

Establish a direct encrypted probe with one explicitly trusted peer

- **ID:** `node direct-probe`
- **Visibility:** visible

### Options

- `--endpoint ENDPOINT` — Peer direct transport address **(required)**
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--peer-node-id PEER_NODE_ID` — Expected canonical peer node ID **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node discovery`

Run one bounded in-memory LAN discovery scan

- **ID:** `node discovery`
- **Visibility:** visible

### Options

- `--include-addresses` — Include observed source addresses in the local CLI result (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--wait-seconds WAIT_SECONDS` — Discovery scan duration in seconds, bounded to 1..=30 (default: `5`)

## `omakure node enroll`

Request and explicitly approve or reject manual enrollment

- **ID:** `node enroll`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`node enroll apply`](#omakure-node-enroll-apply)
- [`node enroll approve`](#omakure-node-enroll-approve)
- [`node enroll reject`](#omakure-node-enroll-reject)
- [`node enroll request`](#omakure-node-enroll-request)

## `omakure node enroll apply`

Apply one authority-signed unattended enrollment bundle

- **ID:** `node enroll apply`
- **Visibility:** visible

### Options

- `--bootstrap-nonce BOOTSTRAP_NONCE` — One-time 16-byte bootstrap nonce as lowercase hexadecimal **(required)**
- `--bootstrap-token-file BOOTSTRAP_TOKEN_FILE` — One-time bootstrap token file. The token is never echoed or persisted **(required)**
- `--bundle-file BUNDLE_FILE` — Exact signed OMEB bundle file. The file is never echoed or persisted **(required)**
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node enroll approve`

Approve one pending request after checking the out-of-band code

- **ID:** `node enroll approve`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Audit actor **(required)**
- `--code CODE` — Out-of-band 16-byte approval code as lowercase hexadecimal **(required)**
- `--confirmed` — Confirm this trust mutation explicitly (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--reason REASON` — Audit reason/evidence **(required)**
- `--request REQUEST_HEX` — Exact signed OMMA request as lowercase hexadecimal bytes **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--transport-certificate TRANSPORT_CERTIFICATE` — Candidate transport certificate as lowercase hexadecimal bytes **(required)**

## `omakure node enroll reject`

Reject one pending request without activating trust

- **ID:** `node enroll reject`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Audit actor **(required)**
- `--confirmed` — Confirm this denial explicitly (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--reason REASON` — Audit reason/evidence **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NODE_ID` — Pending candidate node identifier **(required)**

## `omakure node enroll request`

Create and send one signed manual enrollment request

- **ID:** `node enroll request`
- **Visibility:** visible

### Options

- `--capability CAPABILITIES` — Requested capability (repeatable; sorted unique values are required)
- `--endpoint ENDPOINT` — Peer direct transport address **(required)**
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--lifetime-seconds LIFETIME_SECONDS` — Request lifetime in seconds, at most 30 days (default: `600`)
- `--role ROLE` — Requested peer role (default: `performer`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node health`

Show current fleet health: presence, profile, and runner status

- **ID:** `node health`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node init`

Explicitly initialize public config, identity, and local trust state

- **ID:** `node init`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node peers`

List registered peers without audit history or private state

- **ID:** `node peers`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node reset`

Explicitly remove validated machine identity and node trust state

- **ID:** `node reset`
- **Visibility:** visible

### Options

- `--confirmed` — Confirm destructive removal of identity and trust state (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node revoke`

Revoke one peer with confirmation and evidence

- **ID:** `node revoke`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Audit actor **(required)**
- `--confirmed` — Confirm this trust mutation explicitly (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--reason REASON` — Audit reason/evidence **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `NODE_ID` — Peer node identifier **(required)**

## `omakure node serve`

Run the machine-owned HTTP node service with optional workers and scheduler

- **ID:** `node serve`
- **Visibility:** visible

### Options

- `--allow-non-loopback` — Explicitly allow binding to non-loopback addresses (values: `false`, `true`)
- `--allow-non-loopback-direct` — Explicitly allow the direct transport to bind to non-loopback addresses (values: `false`, `true`)
- `--bind BIND` — Address to bind the HTTP API server to; defaults to node.toml `api.bind`
- `--bootstrap-token-file BOOTSTRAP_TOKEN_FILE` — Node-local one-time bootstrap token file for the signed-bundle API
- `--capability CAPABILITIES` — API capability to grant in legacy single-token mode. Repeatable
- `--direct-bind DIRECT_BIND` — Optional direct transport listener address
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--no-scheduler` — Disable the in-process schedule scanner (default: `false`) (values: `false`, `true`)
- `--policy POLICY` — Deploy-only policy.toml. Same as `omakure api --policy`
- `--readiness-requires-scheduler` — Fail `/v1/ready` when the scheduler is enabled but not alive (values: `false`, `true`)
- `--readiness-requires-transport` — Fail `/v1/ready` while configured static peers are not connected (values: `false`, `true`)
- `--readiness-requires-worker` — Fail `/v1/ready` when configured workers are not alive (values: `false`, `true`)
- `--scheduler` — Explicitly enable the in-process schedule scanner (default: `false`) (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--secret-ref SECRET_REFS` — Allowed secret provider ref for secrets:use. Same as `omakure api --secret-ref`
- `--tokens-file TOKENS_FILE` — Multi-token TOML file. Same as `omakure api --tokens-file`
- `--worker-actor-filter WORKER_ACTOR_FILTER` — Only claim jobs whose actor matches this tag
- `--worker-script-filter WORKER_SCRIPT_FILTER` — Only claim jobs whose script path or name contains this pattern
- `--workers WORKERS` — Number of embedded queue workers. `0` means API-only (no claiming)

## `omakure node signals`

Show the bounded newest-first closed Signal feed: enrolled, revoked, run-completed

- **ID:** `node signals`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node status`

Inspect public node identity, redacted config, and bounded trust counts

- **ID:** `node status`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure node trust`

Explicitly import and activate one manually trusted peer

- **ID:** `node trust`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Audit actor **(required)**
- `--capability CAPABILITIES` — Allowed capability (repeatable; sorted unique values are required)
- `--confirmed` — Confirm this trust mutation explicitly (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--node-id NODE_ID` — Canonical omk1_ node identifier **(required)**
- `--public-key PUBLIC_KEY` — Lowercase hexadecimal x-only BIP-340 public key **(required)**
- `--reason REASON` — Audit reason/evidence **(required)**
- `--role ROLE` — Peer role: conductor or performer (default: `performer`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--transport-certificate TRANSPORT_CERTIFICATE` — Signed transport certificate as lowercase hexadecimal bytes

## `omakure queue`

Push, cancel, drain, and inspect the run queue

- **ID:** `queue`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`queue add`](#omakure-queue-add)
- [`queue cancel`](#omakure-queue-cancel)
- [`queue dead-letter`](#omakure-queue-dead-letter)
- [`queue stats`](#omakure-queue-stats)
- [`queue worker`](#omakure-queue-worker)

## `omakure queue add`

Push a job onto the queue

- **ID:** `queue add`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Actor tag recorded on the row (default: `human`) (default: `human`)
- `--cron-schedule-id CRON_SCHEDULE_ID` — Provenance id tying this row to a named cron schedule. Populated automatically by `omakure serve`; set manually only to replay or simulate a scheduled run
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--parent-run-id PARENT_RUN_ID` — Optional parent run id, for chained agent workflows
- `--priority PRIORITY` — Higher value picked first (default 0) (default: `0`)
- `--reason REASON` — Optional free-form reason
- `--run-id RUN_ID` — Caller-provided run id; otherwise a fresh id is generated
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--timeout TIMEOUT` — Per-job execution timeout (e.g. `30s`, `5m`, `1h`). Without this flag the job has no execution limit
- `ARGS` — Arguments forwarded to the script (after `--`)
- `SCRIPT` — Script name or path **(required)**

## `omakure queue cancel`

Cancel a queued or running job

- **ID:** `queue cancel`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--reason REASON` — Optional reason recorded on the cancelled row
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `RUN_ID` — Run id to cancel **(required)**

## `omakure queue dead-letter`

Promote a `failed` or `timed_out` row into `dead_letter`

- **ID:** `queue dead-letter`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--reason REASON` — Optional reason appended to the row
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `RUN_ID` — Run id to promote **(required)**

## `omakure queue stats`

Aggregate counts per state and per actor

- **ID:** `queue stats`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure queue worker`

Drain the queue (long-running daemon)

- **ID:** `queue worker`
- **Visibility:** visible

### Options

- `--actor-filter ACTOR_FILTER` — Only claim jobs whose actor matches this tag
- `--concurrency CONCURRENCY` — Number of parallel workers (default 1) (default: `1`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--script-filter SCRIPT_FILTER` — Only claim jobs whose script path or name contains this pattern
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure run`

Run a script directly

- **ID:** `run`
- **Visibility:** visible

### Options

- `--actor ACTOR` — Actor tag recorded in the run history (default: `human`) (default: `human`)
- `--env-file PATH` — Path to an env file whose `KEY=value` pairs are injected into the script process for this run only. Values override the managed active env for the same key, but omakure-reserved vars (`OMAKURE_RUN_ID`, `OMAKURE_SCRIPTS_DIR`) always win. A missing or unreadable path is a hard error
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--no-prompt` — Fail with a structured error when required schema fields are missing instead of prompting on stdin or a TTY. Implied by `--json`. Does not disable prompts embedded in the script itself (for example `omakure init` templates that read optional values); for non-interactive runs pass arguments after `--` or use this flag and supply every required value (values: `false`, `true`)
- `--parent-run-id PARENT_RUN_ID` — Optional parent run id, for chained agent workflows
- `--reason REASON` — Optional free-form reason recorded in the run history
- `--run-id RUN_ID` — Caller-provided run id; otherwise a fresh id is generated
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--secret FIELD=VALUE` — Direct secret field input as `FIELD=value`. The value is supplied to secret schema fields for this run and is redacted from stored args
- `ARGS` — Arguments forwarded to the script
- `SCRIPT` — Script name or path **(required)**

## `omakure scripts`

List available scripts

- **ID:** `scripts`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--tag TAG` — Filter by tag (repeatable; AND semantics, case-sensitive literal match against the script's embedded `Tags` field)

## `omakure search`

Search the script index

- **ID:** `search`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--tag TAG` — Filter by tag (repeatable; AND semantics, case-sensitive literal match against the script's embedded `Tags` field)
- `QUERY` — Free-text query (matches name, description, tags, fields) (default: ``)

## `omakure serve`

Run the cron scheduler daemon for scripts declaring a `Schedule` block

- **ID:** `serve`
- **Visibility:** visible

### Options

- `--concurrency CONCURRENCY` — Number of worker threads for the in-process worker (default 1) (default: `1`)
- `-d, --detach` — Run the scheduler as a detached background daemon (Unix only) (values: `false`, `true`)
- `--install` — Install a systemd user service that runs `omakure serve` for the current workspace and survives reboots (Linux only) (values: `false`, `true`)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--no-worker` — Do not spawn the in-process worker. Use when you already run `omakure queue worker` elsewhere (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--status` — Print the systemd user service status for the current workspace (Linux only) (values: `false`, `true`)
- `--stop` — Stop a running daemon (reads `.omakure/daemon.pid` and sends SIGTERM) (values: `false`, `true`)
- `--uninstall` — Disable and remove the systemd user service for the current workspace (Linux only) (values: `false`, `true`)

## `omakure token`

Generate hashed API tokens for `--tokens-file` auth

- **ID:** `token`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

### Subcommands

- [`token generate`](#omakure-token-generate)

## `omakure token generate`

Generate a plaintext token, Argon2id hash, and TOML entry

- **ID:** `token generate`
- **Visibility:** visible

### Options

- `--append APPEND` — Append the TOML entry to this tokens file (requires `--confirmed`)
- `--confirmed` — Confirm a destructive/automated `--append` (values: `false`, `true`)
- `--id ID` — Stable token id (logged/audited; never the secret) **(required)**
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scope SCOPES` — Scope to grant (repeatable), e.g. runs:read, scripts:read, * **(required)**
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure trace`

Run the machine-owned node service (HTTP API + optional workers + scheduler)

- **ID:** `trace`
- **Visibility:** visible

### Options

- `--data DATA` — Optional structured payload (must parse as JSON)
- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--level LEVEL` — Level (debug, info, warn, error). Defaults to `info` (default: `info`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `MESSAGE` — Trace message **(required)**

## `omakure uninstall`

Remove the omakure binary (optionally wipe the scripts workspace)

- **ID:** `uninstall`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--scripts` — Also delete the scripts workspace directory (runs.sqlite, history, schedules, and every user script). Destructive (values: `false`, `true`)
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override

## `omakure update`

Update omakure from GitHub releases

- **ID:** `update`
- **Visibility:** visible

### Options

- `--json` — Emit machine-readable JSON output for AI-facing subcommands (values: `false`, `true`)
- `--repo REPO` — GitHub repository (`owner/name`). Defaults to `$OMAKURE_REPO` / `$OVERTURE_REPO` / `$CLOUD_MGMT_REPO` / `$REPO` / `This-Is-NPC/omakure`
- `--scripts-dir SCRIPTS_DIR` — Scripts directory override
- `--version VERSION` — Release tag to install (e.g. `v0.1.9`). Defaults to `$VERSION` or the latest GitHub release for the configured repo

<!-- END GENERATED CLI REFERENCE -->
