# Deterministic coverage

Omakure uses LLVM source coverage rather than Tarpaulin. The supported coverage
line is Rust `1.97.1`, its matching `llvm-tools-preview` component, and
`cargo-llvm-cov` `0.9.0`. The tool is pinned in `mise.toml` and the runner
verifies the versions before it starts.

## Local run

```bash
mise run coverage
```

The task performs one instrumented run across all normal workspace unit and
integration targets (`--workspace --all-targets --locked`, with doctests and
nightly branch coverage intentionally out of scope). It uses
`CARGO_INCREMENTAL=0`, an isolated target/profile directory,
`RUST_TEST_THREADS=1`, and cargo-llvm-cov's
`LLVM_PROFILE_FILE_NAME=omakure-%m.profraw` override. The latter keeps raw
profiles at cargo-llvm-cov's merge-discovery root, while fixed test-thread
scheduling makes the execution profile deterministic. Paths are remapped to
repository-relative names. Reports are written below
`target/coverage-report/`:

- `html/index.html`
- `lcov.info`
- `cobertura.xml`
- `llvm-cov.json`
- `inventory.json`

If the pinned tool or LLVM component is not already present, the task performs
the explicitly pinned setup step. `mise run coverage:test` is the offline
fixture and never installs a tool.

## Baseline and gate

`scripts/coverage/baseline.json` is reviewed metadata, not generated state. It
records the source revision, toolchain, test/features scope, normalized source
inventory, line/region/function counts, and
`line_threshold_basis_points`. The line gate compares integer counts by cross
multiplication:

```text
covered_lines * 10000 >= coverable_lines * line_threshold_basis_points
```

No floating-point percentage or display rounding participates in the decision.
The threshold is the initial baseline rounded down to two decimal percentage
points. Source additions and deletions are reported in `inventory.json`; they
are accepted only when the new total still passes. Region and function counts
are informational.

Every current `src/**/*.rs` file is retained in the normalized inventory. A file
with no coverable region is represented with zero counts. Paths outside
production source are excluded only with a machine-readable reason (`dependency`,
`generated`, `build-artifact`, or `test-harness`); handwritten `src/**` paths
cannot be excluded. External paths are classified before normalization and
written as `external/<path-hash>-<basename>` so workstation prefixes are hidden
without collapsing duplicate basenames.

Run the focused offline fixtures with:

```bash
mise run coverage:test
```

They prove the exact threshold boundary, one basis point below the boundary, a
seeded uncovered production line, deterministic normalized output, complete
source inventory handling, collision-resistant external exclusions, explicit
classification, rejection of unclassified paths, and runner preflight ordering.

## CI

The `coverage` CI job runs `./scripts/tasks/coverage` for pull requests. The
repository-owned baseline gate is the sole coverage decision, and reports are
generated under `target/coverage-report/`. Fork and same-repository pull
requests use identical local behavior.
