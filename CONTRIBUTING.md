# Contributing

## Commit Standards

- Use [Conventional Commits](https://www.conventionalcommits.org/).
- Write every commit message in English.
- Format: `type(scope): concise summary` or `type: concise summary`.
- Types: `feat`, `fix`, `docs`, `refactor`, `chore`, `test`, `build`, `ci`, `perf`.
- Breaking changes: add `!` after type/scope (e.g., `feat!: remove deprecated API`).

## Branch Naming

- Feature: `feature/{short-name}` (e.g., `feature/audio-badges`)
- Fix: `fix/{short-name}` (e.g., `fix/duplicate-insert`)
- Use lowercase kebab-case. Keep names short and descriptive.

### Default base branch
- **Branch new work off `master`**. `master` is the integration and release
  branch.
- Open pull requests directly against `master`.

## Task & Plan Management

Shaping, planning, and task tracking are handled through the **Omakiten** MCP
(this replaces the former assisted-workflow templates and skills). Use it to
turn ideas into ready tasks and group them into waved execution plans:

- `okt-shape` — shape a raw idea or backlog into prioritized, ready tasks plus a plan.
- `okt-run` — drive a plan to completion; `okt-task-continue <id>` builds one task by hand.
- Tasks, plans, waves, and dependencies are project-scoped — inspect with
  `project.overview`, `tasks.list`, and `plans.show`.

## Project Management

| Setting | Value |
|---------|-------|
| Tool | GitHub Projects (user-scoped) |
| Access method | CLI (`gh`) |
| Project/Board URL | https://github.com/users/This-Is-NPC/projects/6 |
| Project number | `6` |
| Project owner | `This-Is-NPC` |
| Project node ID | `PVT_kwHOBaVOcc4BOEPO` |
| Status field ID | `PVTSSF_lAHOBaVOcc4BOEPOzg84Hmg` |
| Status options | `Todo` = `f75ad846`, `In Progress` = `47fc9ee4`, `Done` = `98236657` |
| Repository | `This-Is-NPC/omakure` |

### How to read tasks

```bash
gh project item-list 6 --owner This-Is-NPC
```

### How to create an issue and add it to the project

Create the issue in the repository, then add it to project 6 and set its status. Pass the body inline via heredoc — never create a throwaway `.md` file.

```bash
# 1. Create the issue (capture the URL printed on stdout)
gh issue create --repo This-Is-NPC/omakure \
  --title "<lowercase action verb + what + where>" \
  --body "$(cat <<'EOF'
<user story body here>
EOF
)"

# 2. Add the issue to the omakure project (returns the item node id as PVTI_...)
gh project item-add 6 --owner This-Is-NPC \
  --url <issue URL from step 1> \
  --format json

# 3. Set status to Todo on the newly added item
gh project item-edit \
  --project-id PVT_kwHOBaVOcc4BOEPO \
  --id <PVTI_... item id from step 2> \
  --field-id PVTSSF_lAHOBaVOcc4BOEPOzg84Hmg \
  --single-select-option-id f75ad846
```

### How to update status

Swap the `--single-select-option-id` in step 3 above:

- `Todo` → `f75ad846`
- `In Progress` → `47fc9ee4`
- `Done` → `98236657`

If you only have the issue URL and not the project item id, first look it up:

```bash
gh project item-list 6 --owner This-Is-NPC --format json \
  | jq -r '.items[] | select(.content.url == "<issue URL>") | .id'
```

## Code Standards

- `mise run lint` must exit 0 before opening a PR. It routes to the canonical
  formatting and Clippy atomics.
- Any new `#[allow(clippy::…)]` requires a one-line comment justifying the
  suppression (pointing at the bug, audit note, or rationale).

## Local checks and hooks

Install the tracked hooks once from the repository root:

```bash
mise run hooks:install
```

The installer writes only this repository's local `core.hooksPath` setting; it
does not modify global Git configuration. Hooks are exact, thin routes:
`pre-commit` executes `scripts/tasks/check/fast`, and `pre-push` executes
`scripts/tasks/check/full`. The checks stop at the first failure and preserve
trap-backed cleanup owned by certification scripts.

The automation layers have deliberately different responsibilities:

- An **atomic** under `scripts/tasks/atomic/` performs one logical operation
  (formatting, one test group, a build, a package check, or a contract).
- A **suite** under `scripts/tasks/suite/` aggregates atomics or retained
  certification scripts. `native-tests` covers the local library, bins,
  examples, docs, and every `tests/*.rs` target once through
  `native-integration`.
- `check/fast` and `check/full` are the only aggregate local gates. Fast is
  the cheap pre-commit gate. Full is the complete locally executable Linux
  gate and does not invoke fast a second time.
- `check/platform/` has four matrix-facing suites:
  `linux-gnu`, `linux-musl`, `macos`, and `windows`. They validate the runner
  and target, then route to native tests, release/build, static-link checks
  where applicable, and binary smoke.

The same call graph is used locally and in hosted CI:

```text
hook -> check/{fast,full} -> atomic/suite
mise task -> one canonical atomic, suite, check, installer, or retained script
CI/release matrix -> check/platform/${platform} ${target}
```

Both checks require the pinned Rust toolchain, Python with PyYAML, and GNU
`timeout`; GNU `timeout` is also required by the fast check. Full additionally
requires Linux, Docker Engine and Compose, `jq`, and SQLite. macOS runners
need an owned physical `RUNNER_TEMP`; Windows runners need the supported
MSVC target and static CRT setup; musl runners need `musl-tools` and
`musl-gcc`. A local Linux host is the only platform on which the complete full
scope is available.

Full includes usage/catalog validation, deterministic coverage, complexity
calibration/ratchet/audit, release packaging, Docker smoke, transport and
Health certification, and cleanup verification. Destructive Fedora VM/KVM
certification is intentionally excluded from automatic pre-push. Run it only
on a prepared host with libvirt and its VM prerequisites:

```bash
mise run cert:vm
```

The static VM policy check remains part of `check:full`; `cert:vm` is the
separate destructive certification entry point.

## Repository layout

This is a headless Rust/HTTP product. Product documentation belongs under
`docs/`; the conventional root exceptions are `README.md`, `AGENTS.md`,
`CONTRIBUTING.md`, `LICENSE`, Cargo/Docker/mise manifests, and the canonical
`compose.yaml`.

All repository automation lives below `scripts/`: canonical user-facing routes
are atomics under `scripts/tasks/atomic/`, suites under `scripts/tasks/suite/`,
and platform gates under `scripts/tasks/check/`. Retained certification and
developer implementations live under `scripts/tasks/cert/` and
`scripts/tasks/dev/`; installers are under `scripts/install/`, release tooling
under `scripts/release/`, non-subject fixtures under `scripts/fixtures/`, and
the isolated debug workspace under `scripts/workspace/`. Do not add a root
script or a repository-owned Battery subject collection. External Battery
repositories own subject scripts; use explicit Battery registration, sync, and
install operations to materialize them into a workspace.

## Mise task policy

Every Mise `run` entry is a direct invocation of one existing executable
repository script. Keep composition in the canonical shell suites rather than
embedding command chains or dependencies in `mise.toml`. File tasks must resolve
the project through `MISE_PROJECT_ROOT` with a direct-invocation fallback.
Declare `sources` and `outputs` only for deterministic local tasks. Builds and
cleanup should be repeat-safe; tests and linters rerun on every invocation.
Install, release, node-service, and live Docker/libvirt certification tasks are
stateful operations: document their preconditions and ensure bounded,
trap-backed cleanup rather than claiming strict idempotence.

## Focused validation

Before submitting, run the narrow checks for changed surfaces first:

```bash
bash -n scripts/tasks/atomic/shell-syntax
scripts/tasks/atomic/shell-syntax
mise run check:fast
mise run test:integration
```

Then run `mise run lint` and the relevant test or certification suite. Do not
hide generated state or Battery-installed scripts with broad ignore patterns.
