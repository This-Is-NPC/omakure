# Refactoring handoff

Updated on 2026-10-01 for branch `refactor/project-review`.

## Current checkpoint

- Continue from `REFACTORING_PLAN.md`, the active project plan. Its checklist has not been synchronized with the implementation yet; verify each item against the code before acting on it.
- Keep one reviewed implementation stage per commit. Use an imperative Conventional Commit subject of at most 50 characters and let the pre-commit `check:fast` hook run.
- Implement stages in isolated agent checkouts, review each patch in the main worktree, run focused tests, and commit only after the review passes.
- The Omakiten registration for `omakure` points to a different checkout. Do not move its tasks or mix project state with this branch until its root is reconciled.

## Validation

- `mise run check:full` passed on `ae70c42`, including Linux GNU tests, overlay tests, 50,877/57,211 line coverage, complexity, Docker smoke, transport certification, Health Plane certification, and cleanup verification. Repeat it after the remaining stages before claiming the branch is fully validated.
- Every stage committed after `ae70c42` passed the pre-commit `check:fast` hook. Focused tests also passed for each stage.
- Docker integration tests for discovery, manual enrollment, and signed bundle passed with ignored tests enabled and one test thread.

## Active work

- Finish the ongoing RunStore boundary work in CLI adapters and the remaining Rust 2024 environment test preparation.
- Continue the plan's architecture, typed-error, complexity, async, test, dependency, toolchain, and CI work. Review each unchecked item against the current tree because several have already been implemented.
- Audit the final tree for unused paths, duplicated behavior, and compatibility branches. Keep documentation about current behavior and architecture only.
- Run `mise run check:fast` and `mise run check:full` on the final tree, then reconcile the plan and prepare the single PR against `master`.
