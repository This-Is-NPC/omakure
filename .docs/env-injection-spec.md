# Environment injection spec

This is the **written contract** that gates all environment-injection and
env-file-parser code. It fixes three decisions that cross-cut every
downstream task so no implementer re-decides them:

1. the **precedence table** — the canonical merge order of env sources;
2. the **var-expansion grammar** — how `$VAR` / `${VAR}` are substituted;
3. the **secret-non-persistence invariant** — injected env reaches the
   spawned process only, never storage.

Downstream tasks implement *from* this document:

- **1753** — env-file parser (`--env-file` / `.conf` reader)
- **1754** — injector (merge + apply precedence, build `extra_env`)
- **1755** — interpreter (var-expansion engine)
- **1756** — CLI surface (`--env-file` flag wiring)

Terminology: "env" means the ordered set of `KEY=value` pairs that will be
handed to a script process. "Merged env" means the single map produced
after all precedence layers have been folded together. Keys are compared
**case-sensitively** at injection time (unlike the TUI-defaults parser in
`src/adapters/environments.rs`, which lowercases keys to match schema
fields — that path is unrelated to process injection and is out of scope
here).

---

## 1. Precedence table

Env is assembled by folding sources in a fixed order, **lowest priority
first, highest priority last**. When two sources define the same key, the
**later (higher-priority) source wins** — its value overwrites the value
already in the merged map. "Win" therefore means *last write of a given
key survives*.

| # | Source | Priority | Overridable by later layers? |
|---|--------|----------|------------------------------|
| 1 | Parent shell env (inherited by the omakure process) | lowest | yes |
| 2 | Managed active env — `.omakure/envs/<name>.conf`, selected via the `active` pointer (`.omakure/envs/active`) | low | yes |
| 3 | CLI `--env-file <path>` | high | yes |
| 4 | Omakure-reserved vars — `OMAKURE_RUN_ID`, `OMAKURE_SCRIPTS_DIR` | highest | **no — non-overridable** |

### Rules

- **Fold order is exactly 1 → 2 → 3 → 4.** Each layer is applied on top
  of the accumulated map from all previous layers.
- **A later source overwrites an earlier key of the same name.** A key
  present only in an earlier source is preserved (it is not cleared by a
  later source that lacks it).
- **Layer 4 is applied after everything else and is non-overridable.**
  The reserved keys `OMAKURE_RUN_ID` and `OMAKURE_SCRIPTS_DIR` are
  injected *last*, so a user variable of the same name from layers 1–3
  **cannot** clobber them. If a user defines `OMAKURE_RUN_ID` in their
  shell or `.conf` or `--env-file`, the omakure value still wins. The
  injector MUST guarantee reserved keys are the final writes into the
  merged map (never merged first and then risk being overwritten).
- Reserved-key values are authoritative:
  - `OMAKURE_RUN_ID` = the run's `run_id`.
  - `OMAKURE_SCRIPTS_DIR` = the workspace root (so nested `omakure trace`
    calls resolve the same `runs.sqlite`).
- This mirrors the existing injection in `src/run_executor.rs`
  (`execute_with_heartbeat`), where the reserved pair is pushed onto the
  `env` vec **after** the caller-supplied `extra_env` (currently lines
  ~103–111). Downstream layers 2 and 3 are new inputs that feed
  `extra_env`; the reserved-last ordering established there is the
  contract and must be preserved.

### Notes / edge cases

- The **session `omakure.conf`** override (see `.docs/environments.md`)
  governs *TUI schema-field defaults*, not process injection, and is out
  of scope for this spec. If a future task folds it into injection, it
  slots between layers 2 and 3 and MUST be specified in a follow-up; it
  is **not** assumed by tasks 1753–1756.
- If `--env-file` is given more than once, the resolution of repeated
  flags (last-wins vs. accumulate) is a CLI decision owned by task 1756;
  whatever it chooses, the *result* still enters the fold at priority 3.

---

## 2. Var-expansion grammar

After the merged env is produced (post-precedence, section 1), each
**value** may contain references to other variables. The interpreter
(task 1755) resolves them with the following grammar.

### 2.1 Substitution model

- **Single pass.** The input string is scanned left-to-right exactly
  once. Substituted output is **not re-scanned** — there is **no
  recursion**. If an expansion produces text that itself looks like
  `$FOO`, that text is emitted literally.
- **Source of values.** References resolve against the accumulated map
  available while user-provided layers are folded: the parent shell env
  (layer 1), then managed active env (layer 2), then optional `--env-file`
  (layer 3). Crucially this **includes the parent shell env (layer 1)**: a
  reference like `$PATH` resolves to the inherited parent value, so a
  self-referencing value such as `PATH=/x/bin:$PATH` prepends to the
  existing PATH rather than referencing the file's own raw value (which
  would double the prefix and leave a literal `$PATH`). References MUST NOT
  be expanded against only the current file's own keys.
- **Reserved vars are not expansion sources.** `OMAKURE_RUN_ID` and
  `OMAKURE_SCRIPTS_DIR` are injected last by `execute_with_heartbeat` after
  layer-2/layer-3 expansion has already completed. A `.conf` or
  `--env-file` value such as `id=$OMAKURE_RUN_ID` therefore expands the
  reference as undefined (`id=`). The reserved key itself is still injected
  afterward and remains non-overridable at process spawn time.
- **Layer order within expansion.** Values are expanded as each user layer is
  folded in (1 → 2 → 3), against the accumulator built so far. A value in
  a higher-priority layer therefore sees the already-expanded value from a
  lower layer for the same key (e.g. an `--env-file` `PATH=/a:$PATH` sees
  the active env's expanded PATH). Undefined references still expand to
  empty (section 2.3).
- Expansion applies to **values only**, never to keys.

### 2.2 Reference forms

Two forms are recognised:

- `$VAR` — bare form. The name is the longest run matching the name
  charset immediately after `$`.
- `${VAR}` — braced form. The name is everything between `{` and the
  next `}`.

**Name charset:** a valid variable name matches
`[A-Za-z_][A-Za-z0-9_]*` (an ASCII letter or underscore, followed by
letters, digits, or underscores). The braced form `${...}` accepts the
**same** charset between the braces.

- For `$VAR`: scanning stops at the first character not in
  `[A-Za-z0-9_]`. Example: in `$FOO/bar`, the name is `FOO` and `/bar`
  is literal.
- A `$` **not** followed by a valid name-start character (letter or
  underscore) or by `{` is emitted **literally** as `$`. Example:
  `$1` → literal `$1`; `$ ` → literal `$ `.
- A malformed brace with no closing `}` (e.g. `${FOO`) is **emitted
  literally** — the interpreter does not treat an unterminated `${` as a
  reference. (Downstream MAY additionally surface a parse warning, but
  the defined behavior is literal passthrough.)

### 2.3 Undefined variables

- A reference to a name **not present** in the merged env expands to the
  **empty string** (`""`). This applies to both `$VAR` and `${VAR}`.
- Example: if `BAZ` is unset, `x${BAZ}y` → `xy` and `a$BAZ` → `a`.

### 2.4 Escaping

- The **only** escape is `\$` → a literal `$`. When the interpreter sees
  a backslash immediately followed by `$`, it emits a single `$` and
  does **not** treat the following characters as a reference.
  - Example: `\$HOME` → literal `$HOME` (no expansion).
- A backslash **not** immediately followed by `$` is **preserved
  verbatim** — there are no other escape sequences. `\n`, `\\`, `\t`,
  etc. are passed through unchanged as the two (or more) literal
  characters they are.
  - Example: `a\b` → `a\b`; `\\` → `\\`; `\\$X` → `\$X` (the first
    `\` is literal, then `\$` emits a literal `$`; the `X` is not
    re-scanned as a reference). To avoid ambiguity, implement escaping as:
    scan for `\$` as a two-character unit first; any other `\` is a
    literal character with no special meaning.

### 2.5 Explicitly forbidden / out of scope

The following are **not interpreted** and MUST NOT be implemented as
active behavior by task 1755:

- **Command substitution** — `$(...)` and backtick `` `...` `` forms are
  **forbidden**. They are treated as literal text (a `$` before `(` is
  not a valid reference start, so `$(date)` emits literally as
  `$(date)`). No subprocess is ever spawned by the expander.
- **Recursion / re-scanning** of expansion output (see 2.1).
- **Rich bash-style expansions**, explicitly out of scope:
  `${VAR:-default}`, `${VAR:=default}`, `${VAR:?err}`, `${VAR:+alt}`,
  `${#VAR}`, `${VAR/foo/bar}`, `${VAR^^}`, indirection `${!VAR}`,
  arithmetic `$((...))`, and any array/parameter operators. If such a
  form appears, the `${...}` body is taken as a *name* and matched
  against the name charset; because these bodies contain characters
  outside `[A-Za-z0-9_]`, they will not match a defined variable and
  therefore resolve per the undefined rule (empty) unless the body
  happens to be a bare valid name. Downstream tasks SHOULD NOT rely on
  any particular handling of these bodies beyond "not a rich
  expansion."

### 2.6 Worked examples

Given user-layer env `FOO=bar`, `EMPTY=` (absent), no expandable reserved
vars, and inherited parent `PATH=/usr/bin:/bin`:

| Input value | Expanded output |
|-------------|-----------------|
| `$FOO` | `bar` |
| `PATH=/x/bin:$PATH` | `/x/bin:/usr/bin:/bin` (parent PATH preserved, no doubling) |
| `${FOO}` | `bar` |
| `$FOO/baz` | `bar/baz` |
| `${FOO}baz` | `barbaz` |
| `$EMPTY` | `` (empty) |
| `pre${EMPTY}post` | `prepost` |
| `\$FOO` | `$FOO` |
| `$(echo hi)` | `$(echo hi)` |
| `` `date` `` | `` `date` `` (backticks literal) |
| `${FOO:-x}` | `` (empty — `FOO:-x` is not a valid name) |
| `id=$OMAKURE_RUN_ID` | `id=` (reserved vars are injected after expansion) |
| `$1abc` | `$1abc` (`$1` literal, then `abc`) |
| `${FOO` | `${FOO` (unterminated, literal) |

---

## 3. Secret-non-persistence invariant

### Statement

> Injected env reaches the spawned process's `os.environ` (the child
> process environment) **at spawn time ONLY**. It is **never** written to
> `runs.sqlite`, daemon logs, or the run trace.

Concretely: the merged env / `extra_env` is passed to
`std::process::Command::env` and then discarded when the child is
spawned. No layer of the persistence stack receives it.

### Why this holds today (cited persistence path)

The env travels a single, narrow path and there is **no storage sink on
it**:

1. **Injection point** — `src/run_executor.rs`,
   `execute_with_heartbeat(...)`: the `extra_env: Vec<(String, String)>`
   parameter is moved into a local `env` vec; the reserved pair is pushed
   (~lines 103–111); the vec is handed to
   `MultiScriptRunner::build_command(&script_path, &args, &env)`
   (~line 113). It is never inserted into any `RunRow` and never passed
   to any `runs::*` writer.

2. **Spawn point** — `src/adapters/script_runner.rs`,
   `MultiScriptRunner::build_command(script, args, env)` (line 27): each
   pair is applied via `cmd.env(k, v)` (lines 35–37). This is the *only*
   consumer of the env. The resulting `Command` is spawned in
   `run_executor.rs` (`command.spawn()`); the env exists solely inside
   the child process from that point on.

3. **Persistence point (env-free by construction)** — `src/runs.rs`,
   `insert_run(conn, row)` (line 548) executes
   `INSERT INTO runs (...)` (lines 550–556). The `runs` table schema
   (`init_schema`, lines 490–515) has columns for
   `args_json, actor, reason, state, ..., stdout, stderr, error,
   omakure_version` and so on — **there is no `env` column and no
   `extra_env` field on `RunRow`.** The env cannot be persisted here
   because there is nowhere to put it. Trace rows go to `run_traces`
   (`init_schema` lines 525–535; writer `INSERT INTO run_traces` at
   `src/runs.rs` ~line 1232) which stores `level, message, data_json` and
   likewise has no env sink.

4. **Masking (defense in depth, TUI preview only)** —
   `src/adapters/environments.rs`, `is_sensitive_key(key)` (line 236)
   flags keys containing `password`, `secret`, `token`, `key`, `api`,
   `private`, or `cred`, and the Environments **preview** masks their
   values with `***`. This is a *display* control for the `.conf` preview
   pane, independent of injection; it is **not** the mechanism that keeps
   secrets out of storage (that is guaranteed structurally by points
   1–3). Downstream code MUST NOT rely on masking as the persistence
   guard.

### Rules for downstream tasks

- **Do not add an `env` / `extra_env` column** to the `runs` table or any
  field carrying env to `RunRow`, `RunCompletion`, or any `runs::*`
  writer.
- **Do not log `extra_env`, the merged env, or individual injected
  values** — not via `println!`/`eprintln!`, not via the trace
  (`run_traces`), not via any structured logger. The env must have
  exactly one consumer: `cmd.env(...)` in `build_command`.
- Any new diagnostic that wants to surface *which keys* were injected MAY
  log **key names only**, never values, and SHOULD apply
  `is_sensitive_key` if it ever prints anything derived from a value.

### Residual exposure (accepted tradeoff)

The invariant bounds *omakure's own* storage. It does **not** eliminate
OS-level exposure, which is **acknowledged and accepted, not fixed**:

- The child's environment is readable via `/proc/<pid>/environ` by the
  same user (and root) while the process lives.
- The environment is **inherited by grandchild processes** the script
  spawns.

These are inherent to passing secrets through process environment and are
an accepted tradeoff for this feature. Callers who cannot tolerate this
exposure should not route secrets through env injection.

---

## Acceptance checklist (for downstream reviewers)

- [ ] Injector folds sources in order 1→2→3→4; later layer wins per key.
- [ ] `OMAKURE_RUN_ID` / `OMAKURE_SCRIPTS_DIR` applied last and cannot be
      overridden by layers 1–3.
- [ ] Expander is single-pass, non-recursive, sourced from merged env.
- [ ] Undefined → empty string; `\$` is the only escape; no command
      substitution; no `${VAR:-default}` support.
- [ ] No `env` column / field added to persistence; `extra_env` never
      logged or traced; `cmd.env` is its sole consumer.
