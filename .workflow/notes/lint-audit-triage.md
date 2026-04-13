# Lint & Bug Audit — Triage Table

Companion artifact for `.workflow/plans/lint-and-bug-audit.md`. Each row of
`cargo clippy --all-targets -- -D warnings` (baseline) and
`cargo clippy --all-targets -- -W clippy::pedantic` (audit) is classified as:

- **bug** — wrong runtime behavior today; fixed in this PR.
- **latent-risk** — narrow input range or platform-dependent overflow that is
  not reachable today but is defensively fixed.
- **cosmetic** — style or precision-loss warning with no behavioral impact;
  deliberately not addressed. Rationale recorded inline.

## Baseline warnings (`cargo clippy --all-targets -- -D warnings`)

All 6 rows were pre-existing and blocked the strict gate. Each is resolved by
structural change (no `#[allow(...)]`).

| File:line (pre-fix) | Lint | Label | Fix |
|---|---|---|---|
| `src/adapters/tui/events.rs:210` | `clippy::items-after-test-module` | cosmetic-blocking | `fn handle_envs_key` moved above `#[cfg(test)] mod tests`. |
| `src/adapters/tui/theme.rs:353` | `clippy::items-after-test-module` | cosmetic-blocking | `fn selection_symbol_str` moved above `#[cfg(test)] mod tests`. |
| `src/adapters/tui/ui.rs:236` | `clippy::items-after-test-module` | cosmetic-blocking | `fn color_to_tuple` moved above `#[cfg(test)] mod tests`. |
| `src/cli/describe.rs:169` | `clippy::items-after-test-module` | cosmetic-blocking | `pub fn sample_envelope` moved above `#[cfg(test)] mod tests`; doc-comment preserved. |
| `src/cli/history.rs:589` | `clippy::field-reassign-with-default` | cosmetic-blocking | Replaced `RunStats::default()` + field assignments with struct-literal init. |
| `src/runs.rs:1909` | `clippy::while-let-loop` | cosmetic-blocking | `loop { match … { Some(x) => …, None => break } }` → `while let Some(x) = …`. |

"cosmetic-blocking" = the lint itself is cosmetic/structural but is pinned at
deny-warnings level, so it blocks the gate; the fix does not change runtime
behavior.

## Pedantic audit — bug / latent-risk rows

| File:line (pre-fix) | Lint / pattern | Label | Fix |
|---|---|---|---|
| `src/adapters/tui/app.rs:245-253` (`scroll_env_preview`) | `u16 as i16` truncation → `u16::MAX as i16 == -1`, so `next > u16::MAX as i16` guard always fires for any non-negative sum | **bug** | Rewritten using `i32` arithmetic + `saturating_add` + `clamp(0, u16::MAX)`. 3 regression tests added (in-range advance, saturate at `u16::MAX`, saturate at 0). |
| `src/adapters/tui/app.rs:569-576` (`scroll_run_output`) | `(-delta) as u16` when `delta == i16::MIN` invokes UB-adjacent unary-negation overflow; `(delta as u16)` when `delta > 0` is safe only because `delta` bit-pattern fits in u16 | **latent-risk** | Same `i32` saturating-clamp pattern as `scroll_env_preview`. 2 regression tests added: `i16::MIN` no-panic → saturates to 0; `i16::MAX` from `u16::MAX-5` saturates to `u16::MAX`. |

## Pedantic audit — cosmetic rows (deliberately not addressed)

Rationale is listed once per family; locations are enumerated.

### `as isize` / `as usize` on `Vec::len()` / selection indices

Sites: `src/adapters/tui/app.rs:213,214,220,260,261,267,310,311,317,376,377,383,449,450,457`.

`Vec::len()` is guaranteed by Rust's allocation invariant to be ≤ `isize::MAX`
on all supported platforms — the allocator refuses to return a region larger
than that. Selection indices are always clamped to `[0, len-1]` before being
cast back to `usize`. Therefore every `usize ↔ isize` round-trip here is
lossless in practice. The warnings are stylistic preferences for explicit
`try_from`/saturating conversions.

**Decision:** cosmetic; no fix. Any future refactor that introduces
`u32`-sized-collection semantics (e.g. WASM target or similar) would have to
revisit.

### `usize as u16` in TUI layout code (row/column heights)

Sites: `src/adapters/tui/ui.rs:44`.

`info_lines` is built by `environment::status_info` which returns a
deterministic, short list (≤ ~10 lines). `u16::MAX = 65535` is two orders of
magnitude above any realistic layout height.

**Decision:** cosmetic; no fix.

### `usize/u64/u128 as f64` in gauge / percentile / average math

Sites: `src/adapters/tui/ui.rs:214,227`, `src/adapters/tui/widgets/dashboards.rs:70,76,77,78,79,275,380,383,752`.

`f64`'s 52-bit mantissa is exact for integers up to 2^52 ≈ 4.5×10^15. Gauge
values (run counts, durations in ms) never approach that bound. Color-lerp
precision (0–255) is trivially within `f32` range.

**Decision:** cosmetic; no fix.

### `f64 as u8/u16/u32/u64` in gauge rendering

Sites: `src/adapters/tui/ui.rs:227`, `src/adapters/tui/widgets/dashboards.rs:71,72,79,275,283,380,383,391`.

All sites compute a ratio ∈ [0,1] or a small integer bucket before casting
back to an integer. The upstream math is bounded so truncation is intentional
(rounding to the nearest renderable integer).

**Decision:** cosmetic; no fix.

### `i64 as u64` / `u128 as u64` in `dashboards.rs`

Sites: `src/adapters/tui/widgets/dashboards.rs:156` (`d as u64` guarded by
`d >= 0`), `:172` (`(sum_u128 / len_u128) as u64` — average ms).

`:156` is guarded by an explicit `if d >= 0` check immediately above, so the
cast is sign-safe. `:172` can only truncate if the *average* duration exceeds
`u64::MAX` milliseconds (~584 million years); unreachable.

**Decision:** cosmetic; no fix.

### Cast family in `src/runs.rs`

~40 pedantic cast warnings across `src/runs.rs` (durations, row counts, SQLite
`i64 ↔ u64` bridge). Every call site either:
- reads a SQLite `INTEGER` (which is stored as `i64`) back into a `u64` domain
  type where the producer guarantees non-negative values (enqueued-at
  timestamps, durations); or
- computes an aggregate in `u128` and truncates to `u64` for a domain field
  whose semantic range is well below `u64::MAX`.

No site was identified where a realistic input can trigger truncation.

**Decision:** cosmetic; no fix. A follow-up ticket could introduce a
`DurationMs(u64)` newtype bridging `i64 ↔ u64` with `try_from` — out of scope
for this audit.

### Non-cast cosmetic pedantic warnings

`uninlined_format_args` (238), `redundant_closure` (14), `missing-backticks-in-docs`
(16), `match-same-arms` (8), `unnested_or_patterns` (15), etc.

Per the requirements §Out of scope ("Rewriting the entire codebase to be
`clippy::pedantic` clean. Cosmetic pedantic warnings … stay as warnings unless
they overlap with a real bug"), these are not addressed.

**Decision:** cosmetic; no fix.

### `unwrap()` in non-excluded production code

Reviewed via `rg '\.unwrap\(\)' src/ -g '!**/tests/**'` after excluding the
files listed in the requirements (`cli/{trace,doctor,uninstall,omaken,update}.rs`).
Remaining `unwrap()` calls are in test-code blocks (`#[cfg(test)] mod tests { … }`)
or on `Mutex::lock()` / `Option` values with a statically-guaranteed `Some`
(post-selection-clamp indexing). No site was classified as bug or latent-risk.

**Decision:** cosmetic; no fix.

## Summary

| Label | Count | Status |
|---|---|---|
| bug | 1 | fixed + 3 regression tests |
| latent-risk | 1 | fixed + 2 regression tests |
| cosmetic (baseline 6) | 6 | fixed structurally (to clear the gate) |
| cosmetic (pedantic, by family) | ~500+ | deliberately deferred, rationale above |

Post-fix state:
- `cargo clippy --all-targets -- -D warnings` → exit 0.
- `cargo clippy --all-targets -- -W clippy::pedantic` → 0 `bug`/`latent-risk`
  rows remaining; only families classified as `cosmetic` above.
