# Batteries

A Battery is an external Omakure-compatible automation repository registered
with a local Omakure workspace. Battery repositories are untrusted input until a
user explicitly installs selected scripts into the trusted scripts workspace.

This document defines the Battery v1 contract. The HTTP management API is a
transport adapter over the same Battery operations as the CLI, with the extra
restriction that HTTP Battery registration and HTTP-triggered Battery use are
limited to `https://` sources.

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

`batteries.json` is the registry. It is versioned so future schema changes can
be migrated deliberately.

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
      "last_synced_at": "2026-07-07T00:00:00Z"
    }
  ]
}
```

Rules:

- `name` is the stable Battery id used by CLI and operation requests.
- `cache_path` is stored relative to the workspace root.
- `resolved_commit` is empty until the first successful sync.
- Writes to the registry should be atomic where practical.
- A malformed registry is an operation error, not a silent reset.

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

Script entries must point at `.bash`, `.sh`, `.ps1`, or `.py` files that contain
a valid Omakure schema block. Paths are always relative to the Battery checkout.

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
workspace by copying it from the validated cache checkout.

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
- A forced overwrite should write through a temporary file and rename into
  place where practical.

## Remove Contract

`battery remove` unregisters the Battery. It does not delete already installed
scripts by default because installed scripts are trusted workspace content after
materialization.

Rules:

- Registry entry removal is required.
- Cache deletion happens only with `--remove-cache`.
- Installed script deletion is out of scope for v1.
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
