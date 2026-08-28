# Environment documents

Managed environment defaults live in `.omakure/envs/*.conf`. The active file
name is stored in `.omakure/envs/active`.

## File format

Each non-comment line is `KEY=value`. Blank lines, `#`/`;` comments, optional
`export ` prefixes, and quoted values are supported. Schema-field matching is
case-insensitive; process environment injection preserves key case.

```text
HOST=prod.example.com
REGION=eastus
API_KEY=secret://prod/api_key
```

Names are plain values such as `dev` or `prod`. `active`, dot-prefixed names,
names ending in `.conf`, path separators, and `..` are rejected.

## CLI and HTTP management

```bash
omakure env list
omakure env create prod HOST=prod.example.com API_KEY=secret://prod/api_key
omakure env show prod
omakure env set prod REGION=eastus
omakure env remove prod API_KEY
omakure env replace prod HOST=prod.example.com REGION=eastus
omakure env activate prod
omakure env deactivate
omakure env delete prod
```

`show` masks sensitive keys containing `password`, `secret`, `token`, `key`,
`api`, `private`, or `cred`. The HTTP API exposes the same operations under
`/v1/envs` with bearer authentication and JSON bodies. Writes are validated and
applied only under the managed environments directory.

## Runtime injection

The active environment is merged into every direct, queued, and scheduled child
process. `omakure run --env-file PATH` adds a one-run layer. Precedence from
lowest to highest is:

1. parent process environment;
2. managed active environment;
3. `--env-file` for direct runs;
4. Omakure-reserved `OMAKURE_RUN_ID` and `OMAKURE_SCRIPTS_DIR`.

Values support single-pass `$VAR` and `${VAR}` expansion. Undefined variables
become empty; `\$` is literal; command substitution and recursive expansion
are not supported. Injected values reach the child at spawn time and are never
written to `runs.sqlite`, logs, or traces. See `internal/env-injection-spec.md` for the
grammar and persistence invariant.

## Interpreter selection

Interpreter lookup uses the merged `PATH`. `omakure config` reports the
resolved absolute interpreter path, which is useful for virtual environments:

```text
VIRTUAL_ENV=/home/me/project/.venv
PATH=/home/me/project/.venv/bin:$PATH
```

The same mechanism applies to Bash, PowerShell, Python, and other tools called
by scripts.

## Secret references

Prefer `secret://provider/key` references over plaintext values. File-backed
providers resolve from managed environment files; `secret://env/NAME` refers to
an explicitly allowed process environment value. HTTP queued runs require
reconstructable references for secret fields because workers must not persist
plaintext. Stored args retain `<redacted>` or the provider reference, never the
resolved secret.

## Template

Copy `.omakure/envs/env_template.conf` to a named `.conf` file, edit it, then
activate it with `omakure env activate NAME`. Use `omakure env show NAME` or the
authenticated HTTP `GET /v1/envs/NAME` to inspect a masked view.
