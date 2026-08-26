# Headless release notes

This document is the current release contract for the headless baseline. It is
separate from historical version notes so old TUI/theme behavior remains
accurately recorded without being presented as current product guidance.

## Product

- Supported surfaces: CLI, HTTP management API, and machine-owned `node serve`.
- CLI discovery: `--help` and `help-ai` are generated from the compiled clap
  command tree.
- Operational probes: unauthenticated `/v1/health` and `/v1/ready`.
- Execution: Bash, PowerShell, and Python scripts with embedded schemas,
  SQLite run state, queue workers, traces, and cron scheduling.
- Deployment: scoped tokens-file auth is preferred; legacy local token auth is
  retained for migration.
- Node transport: direct encrypted transport, trust-neutral discovery, manual
  enrollment, and signed-bundle enrollment are shipped. Nostr, campaigns, and
  MDM remain outside this release.
- Health Plane: Profiles, Pulses, and the closed three-kind Signal feed are
  shipped.
- Remote Cues: shipped, default off, behind five fail-closed gates read only
  from the receiving node's own registry and configuration. A Cue names a script
  the Performer already declared and never carries one, runs with an explicit
  deny-all secret policy, executes at most once, and reports its provenance as
  `cue`. Baseline push, campaigns, and fan-out remain outside this release.

## Intentional breaking removals

The TUI, positional path mode, `theme` command/configuration/assets, Omarchy
theme adapter, directory `index.lua` widgets, Lua widget runtime, and related
developer tasks are not release artifacts or supported contracts. See
`headless-migration.md` for operator actions.

## Release gate

Before publishing a version, CI must pass:

```bash
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
cargo test --test packaging_smoke --locked
cargo test --test direct_transport_contract --locked
cargo test --test direct_transport_e2e --locked
cargo tree --edges normal
mise run transport-certification
mise run health-plane-certification
```

`mise run health-plane-certification` is the Health Plane release gate. It runs
on hosted Linux, where Docker networking is available. macOS and Windows CI keep
honest native coverage instead — the frozen contract vectors, the bounded state
and migration suite, the closed Signal lifecycle, and the multi-node reporting
path that runs in-process — and never claim the multi-container result.

The dependency tree must not reintroduce `ratatui`, `crossterm`, `rattles`, or
`mlua` for the headless baseline. Release archives contain only the platform
binary. The matching `release-notes/vX.Y.Z.md` remains the versioned historical
record required by the release workflow; this document records the current
compatibility boundary.

## Certification Snapshot

Task #2678 measured the post-removal Linux release locally with the same target
and profile as the baseline: `x86_64-unknown-linux-gnu` and Cargo's default
`release` profile (`cargo build --release --locked --target
x86_64-unknown-linux-gnu`).

| Measure | Before | After | Delta |
|---|---:|---:|---:|
| `omakure` release binary | 10,520,464 bytes | 8,815,352 bytes | -1,705,112 bytes (-16.21%) |
| Direct normal dependencies | 27 | 23 | -4 |

The historical task #2678 snapshot records `816 passed`; current local verification
passed: `cargo test --all-targets --locked` (863 passed,
0 failed, twice), `mise run lint`, `cargo test --test packaging_smoke --locked`,
Compose/workflow YAML parsing, lifecycle regression tests, stale-engine negative
scans, and two consecutive bounded transport certification runs. The local
Docker image/topology evidence does not replace hosted macOS/Windows execution.
Historical binary-size and dependency measurements above remain unchanged; the
release archive contract is still binary-only.

Hosted Linux, macOS, and Windows CI/release runs remain pending. They are not
claimed by this local snapshot; task #2678 remains blocked on that evidence.

The snapshot above predates the version-only `0.3.0` bump. The resulting local
Linux `v0.3.0` release binary measured 8,812,016 bytes and its binary-only
archive measured 3,379,153 bytes; these figures are not substituted into the
post-removal comparison above.

Roadmap item 5 added `.lua` as a script kind backed by a Lua runtime embedded in
the binary. Measured locally on Linux, the release binary grew from 13,991,000
to 14,491,152 bytes, a delta of 500,152 bytes. As above, these figures are
appended rather than substituted into the earlier comparison.

This deliberately reverses one trade recorded earlier in this document, which
credited *removing* `mlua` as a bundle win. That removal was correct for the TUI
widget runtime and it stays removed. Half a megabyte is the price of a control
plane that can execute automation on a node with no interpreter installed, which
`rebuild-omakure.md` requires; the win was real, and it is being spent on
purpose.

A caveat worth knowing if you re-measure: the number only appears once something
references the runtime. At the commit that added the dependency alone, the
linker discarded it entirely and the binary came out 1,688 bytes *smaller*, with
zero `lua_` symbols. Check `nm target/release/omakure | grep -c lua_` before
trusting a delta.
