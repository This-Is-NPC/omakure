# Batteries

A Battery is an external Omakure-compatible automation repository registered
with a local Omakure workspace. Battery repositories are untrusted input until a
user explicitly installs selected scripts into the trusted scripts workspace.

This document defines the Battery v1 contract. The HTTP management API is a
transport adapter over the same Battery operations as the CLI, with the extra
restriction that HTTP Battery registration and HTTP-triggered Battery use are
limited to `https://` sources.
Battery installation is currently Unix-only.

## Scope

Battery v1 supports these commands:

- `omakure battery list`
- `omakure battery add <git-url> --ref <ref> --name <name>`
- `omakure battery sync <name>`
- `omakure battery inspect <name>`
- `omakure battery scripts <name>`
- `omakure battery install <name> <script-id> [--force]`
- `omakure battery remove <name> [--remove-cache]`

All commands support the global `--json` flag.

## Non-Goals

- No Omaken compatibility layer, migration, alias, or fallback behavior.
- No direct execution from a Battery cache checkout.
- No submodule checkout by default.
- No manifest generator, hook, or repository-provided code execution during
  inspect, sync, script listing, or install.

## Workspace Storage

Battery metadata is Omakure-owned runtime state and lives under `.omakure/`.

```text
<workspace>/
├── .omakure/
│   ├── batteries.json
│   └── batteries/
│       └── cache/
│           └── <battery-name>/
├── .history/
└── omakure.toml
```

`batteries.json` is the registry. The current registry format has
`"version": 1`.

```json
{
  "version": 1,
  "batteries": [
    {
      "name": "azure",
      "git_url": "https://example.invalid/azure-battery.git",
      "requested_ref": "main",
      "resolved_commit": "0123456789abcdef0123456789abcdef01234567",
      "cache_path": ".omakure/batteries/cache/azure",
      "last_synced_at": "2026-07-07T00:00:00Z",
      "auth": {
        "method": "https_token_ref",
        "token_ref": "secret://creds/git_token"
      }
    }
  ]
}
```

Rules:

- `name` is the stable Battery id used by CLI and operation requests.
- `cache_path` is stored relative to the workspace root.
- `resolved_commit` is empty until the first successful sync.
- Optional `auth` stores HTTPS private-clone metadata only: `method` +
  `token_ref` (`secret://…`). Resolved plaintext tokens never enter the
  registry.
- Writes to the registry are performed through the Battery operations layer.
- A malformed registry is an operation error, not a silent reset.

## Private HTTPS auth

Private HTTPS Batteries authenticate with a `token_ref` pointing at a
`secret://provider/key` (env or managed file provider). Sync resolves the
secret at clone/fetch time via a temporary `GIT_ASKPASS` helper:

- `GIT_TERMINAL_PROMPT=0`
- credential helpers disabled (`credential.helper=`)
- askpass script + token files written `0600` (dir `0700`) and removed after use
- resolved tokens redacted from git stderr/stdout before surfacing errors

Git URLs must not embed credentials (`https://user:pass@…` is rejected).

SSH deploy-key Battery auth is out of scope for this release.

### Secret ACL gaps

HTTP Battery auth reuses the process `--secret-ref` allow-list plus
`credentials:use`. Remaining gaps vs a full plan ACL:

- No per-Battery or per-script target binding beyond the shared ref list
- No delivery-channel ACL (run vs battery) enforced at resolve time —
  metadata advertises `allowed_targets` as informational only
- CLI `battery add/sync` still uses unrestricted local secret access
  (`SecretAccess::allow_all`); HTTP enforces scopes + refs

## Repository Format

The repository root must contain `omakure-battery.toml`.

```toml
[battery]
name = "azure"
version = "0.1.0"
description = "Azure automation scripts for Omakure"

[[scripts]]
id = "azure.rg-list-all"
path = "scripts/azure/rg-list-all.sh"
description = "List Azure resource groups"
tags = ["azure", "resource-groups", "read-only"]
```

Script entries must point at `.bash`, `.sh`, `.ps1`, `.py`, or `.lua` files that
contain a valid Omakure schema block. Paths are always relative to the Battery
checkout.

## Safety Contract

Battery repositories are untrusted. Operations must enforce these rules before
exposing or installing any script:

- Clone/fetch into `.omakure/batteries/cache/<name>`, never into the executable
  scripts workspace.
- Resolve the requested branch/tag/ref to a commit SHA and checkout a detached
  commit before inspection.
- Disable submodules by default.
- Do not execute Git hooks, manifests, generators, scripts, or repo-provided
  code.
- Canonicalize every path and confine it to the cache root.
- Reject absolute paths and `..` traversal.
- Reject symlinks for v1.
- Reject unsupported script extensions and scripts without valid schema blocks.
- Refuse direct execution from the cache.
- Do not inject Omakure management tokens or reserved runtime variables into
  Battery Git or inspect operations.

## Install Contract

`battery install` materializes one selected script into the trusted scripts
workspace by copying it from the validated cache checkout. It is a local act:
it may be initiated by the local CLI or by an authenticated HTTP request to the
install route, but never by a peer or by a Remote Cue. Non-Unix platforms reject
the operation until their no-follow install protections are implemented.

Fleet code delivery uses a separately signed Baseline; a Remote Cue may select a
Battery-installed script only after the receiving node explicitly declares that
Battery in `trust.remote_cue_batteries`.

Rules:

- The default target path is the manifest script path relative to the scripts
  workspace.
- Existing files are never overwritten unless `--force` is set.
- Parent directories are created as needed.
- The copied script preserves its schema block.
- Installation records provenance where practical in a sidecar file under
  `.omakure/batteries/installed/`, keyed by Battery name and script id.
- A failed install must not leave a partial target file when the target did not
  previously exist.
- A forced overwrite replaces the existing target only after the selected
  Battery script has passed validation.

## Remove Contract

`battery remove` unregisters the Battery. It does not delete already installed
scripts by default because installed scripts are trusted workspace content after
materialization.

Rules:

- Registry entry removal is required.
- Cache deletion happens only with `--remove-cache`.
- Removing a Battery does not delete installed scripts.
- Removing an unknown Battery returns a stable not-found operation error.

## Operation Boundary

CLI code translates arguments into operation request DTOs and renders operation
responses. It must not duplicate Battery business logic.

Operations own:

- request validation
- registry/cache reads and writes
- manifest validation
- safety checks
- stable error codes and structured error context

The initial operation error taxonomy is intentionally small:

- `invalid_input`
- `not_found`
- `already_exists`
- `not_synced`
- `manifest_invalid`
- `unsafe_path`
- `unsupported_script`
- `conflict`
- `git_failed`
- `io_failed`
- `registry_invalid`

CLI JSON output uses the existing envelope shape where practical:

```json
{ "ok": true, "data": {}, "error": null, "schema_version": "1" }
```

## HTTP Adapter

The HTTP layer is a transport adapter over these same operations. It does not
shell out to `omakure` and does not reimplement Battery safety logic in route
handlers. HTTP applies a narrower source policy than the local CLI: Batteries
created or used through HTTP must have `https://` Git URLs. Local paths,
`file://`, and plaintext `http://` sources remain CLI-only.

Private HTTPS registration (`token_ref`) additionally requires:

- deploy policy `sources.allow_private_https_batteries = true`
- token scopes `batteries:add|sync` (or `batteries:write`) **and**
  `credentials:use`
- `token_ref` allowed by the process `--secret-ref` ACL
