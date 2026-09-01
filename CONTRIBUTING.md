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

- `mise run lint` must exit 0 before opening a PR. It runs
  `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check`.
- Any new `#[allow(clippy::…)]` requires a one-line comment justifying the
  suppression (pointing at the bug, audit note, or rationale).

## Local checks and hooks

Install the tracked hooks once from the repository root:

```bash
mise run hooks:install
```

The installer writes only this repository's local `core.hooksPath` setting; it
does not modify global Git configuration. The pre-commit hook runs
`scripts/tasks/check/fast`, which runs the shared static/fixture checks once
followed by bounded formatting, Clippy, and locked library tests. The pre-push
hook runs `scripts/tasks/check/full`, which runs the same shared checks once
followed by the canonical bounded formatting/Clippy script, bounded
all-target tests, and the complete locally executable Linux suite: usage and
catalog checks, deterministic coverage, complexity
calibration/ratchet/audit, release packaging, Docker smoke, transport and
Health certification, and Health cleanup verification. It does not invoke
fast, so the library test is not duplicated. Transport certification owns its
retained-suite cleanup verification and intentionally runs its own
`direct_transport_e2e` certification-context test a second time.

Both hooks stop at the first failing command and return that failure status;
cleanup owned by a certification script still runs through its trap. The full
check is intentionally a long-running pre-push gate because it includes
instrumented coverage and bounded multi-container certifications. Do not
advertise bypassing the hooks; fix the reported prerequisite or check failure
instead.

Both checks require the pinned Rust toolchain, Python with PyYAML, and GNU
`timeout`; GNU `timeout` is also required by the pre-commit fast check. The
full check additionally requires Linux, Docker Engine and Compose, `jq`, and
SQLite. Destructive
Fedora VM/KVM certification is intentionally excluded from automatic
pre-push. Run its explicit task when the host, libvirt, and VM prerequisites
are available:

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

All repository automation lives below `scripts/`: executable mise tasks are
under `scripts/tasks/`, installers under `scripts/install/`, release tooling
under `scripts/release/`, non-subject fixtures under `scripts/fixtures/`, and
the isolated debug workspace under `scripts/workspace/`. Do not add a root
script or a repository-owned Battery subject collection. External Battery
repositories own subject scripts; use explicit Battery registration, sync, and
install operations to materialize them into a workspace.

## Mise task policy

Prefer one atomic logical operation per task and express composition with
`depends`, not shell `&&` chains. File tasks must resolve the project through
`MISE_PROJECT_ROOT` with a direct-invocation fallback. Declare `sources` and
`outputs` only for deterministic local tasks. Builds and cleanup should be
repeat-safe; tests and linters rerun on every invocation. Install, release,
node-service, and live Docker/libvirt certification tasks are stateful
operations: document their preconditions and ensure bounded, trap-backed
cleanup rather than claiming strict idempotence.

## Focused validation

Before submitting, run the narrow checks for changed surfaces first (for
example `bash -n` on changed shell tasks, `mise tasks`, and the affected
packaging/config tests), then the required `mise run lint` and relevant test
suite. Do not hide generated state or Battery-installed scripts with broad
ignore patterns.
