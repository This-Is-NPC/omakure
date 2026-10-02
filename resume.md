# Project handoff

Updated: 2026-10-01

Branch: `refactor/project-review`

Validated source revision: `8a79011`

## Current state

Omakure is a headless Rust automation runner with CLI, authenticated HTTP, and machine-owned node service surfaces. `docs/internal/architecture.md` is the current module map and boundary reference. The source stages have been independently reviewed and committed.

The integration suite has 32 Cargo test targets. `scripts/tasks/suite/native-integration` lists each target, and the packaging contract verifies that grouped test sources are included exactly once. Docker and certification targets retain their direct names.

## Validation

| Gate | Result |
| --- | --- |
| `mise run check:fast` | Passed; 1,309 library tests |
| `mise run check:full` | Passed on `8a79011`, including Linux GNU overlay tests, deterministic coverage, complexity, packaging, Docker smoke, transport certification, Health Plane certification, and cleanup verification |
| Standalone line coverage | 51,743 / 57,916 (89.34%) |
| Full-gate line coverage | 51,920 / 57,916 (89.65%) |
| Coverage threshold | 88.50%; all 383 current `src/**/*.rs` files appear in the measured inventory |

## Comparison with the starting revision

| Measure | `b418789` | Validated source |
| --- | ---: | ---: |
| Largest Rust source file | 6,406 lines | 1,143 lines |
| Highest cyclomatic complexity | 49 | 44 |
| Highest cognitive complexity | 95 | 36 |
| Measured line coverage | 86.84% | 89.34% standalone |
| Integration test targets | 33 | 32 |

The local `check:fast` and hosted CI timings were not measured under comparable conditions. Hosted CI can be evaluated after the branch is published and its pull request is opened.

## Next action

Publish this branch and open the single pull request against `master` after approval, then confirm the hosted platform matrix.
