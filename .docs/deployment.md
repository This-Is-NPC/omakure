# Deployment guide

How to run Omakure's HTTP management surface on a single host: API-only,
API plus a separate worker, or the combined `omakure engine` process —
including the official container image.

This guide is for **trusted internal** deployments (loopback or private
network). It is not a public-internet threat model.

## Auth status (read this first)

**Preferred:** multi-token file via `--tokens-file` / `OMAKURE_TOKENS_FILE`
(Argon2id hashes, per-token scopes, `omakure token generate`, SIGHUP reload).
See `.docs/http-api.md` → Authentication Contract.

**Legacy:** `OMAKURE_API_TOKEN` still works when no tokens file is set
(internal id `legacy`, scopes `*`, gated by process-wide `--capability`),
unless deploy policy sets `auth.legacy_env_token = false`.

## Deploy policy (`policy.toml`)

Deploy-only file via `--policy` / `OMAKURE_POLICY_FILE`. **Not** workspace
`omakure.toml`. Bad/missing required policy fails **before** binding a socket.

### Load order (api / engine)

1. Built-in defaults (route groups allowed; engine workers=1, scheduler on;
   `auth.legacy_env_token = true`).
2. Deploy `policy.toml` overlays `[http]` / `[engine]` defaults and hard
   `[routes]` / `[auth]` gates.
3. Explicit CLI flags win when provided (`--bind`, `--workers`,
   `--scheduler` / `--no-scheduler`, readiness flags, `--tokens-file`,
   `--allow-non-loopback`).
4. Workspace `omakure.toml` is never consulted for deploy policy.

Deploy route groups are checked **before** token scopes: a `*` token cannot
override `routes.writes = false` or `routes.battery = false`.

### Schema (v1)

Sections: `http`, `engine`, `routes`, `sources`, `scripts`, `envs`, `runs`,
`secrets`, `auth`. Route groups and auth/engine/http defaults apply globally;
HTTP handlers also enforce `sources.*`, `scripts.*` size/tree limits,
`envs.http_manage` / `envs.allow_secret_refs`, `runs.allow_*`, and
`secrets.metadata_endpoint`. CLI local Battery add is not gated by deploy
`sources.allow_local_batteries` (that key is HTTP/deploy-only).

```toml
version = 1

[http]
enabled = true
bind = "0.0.0.0:7878"          # used when CLI --bind is left at default
allow_non_loopback = true
body_limit_bytes = 1048576
cors = "disabled"              # only "disabled" in v1

[engine]
workers = 2                    # used when --workers omitted
scheduler = true               # used when neither --scheduler nor --no-scheduler
readiness_requires_worker = true
readiness_requires_scheduler = true

[routes]
read = true
writes = true
battery = false
battery_install = false
run_enqueue = true
run_cancel = true
run_dead_letter = false
config = true
doctor = true
envs = true

[sources]
allow_https_batteries = true
allow_local_batteries = false
allow_private_https_batteries = true
allow_private_ssh_batteries = false

[scripts]
max_content_bytes = 1048576
tree_entry_limit = 1000

[envs]
http_manage = true
allow_secret_refs = true

[runs]
allow_env_selection = true
allow_secret_fields = true

[secrets]
provider = "file"
metadata_endpoint = false

[auth]
tokens_file = "/run/secrets/omakure_tokens.toml"
legacy_env_token = false
```

### Hard gates

| Setting | Effect |
|---------|--------|
| `routes.writes = false` | All write methods `403` (even `*` token) |
| `routes.battery = false` | All `/v1/batteries…` `403` |
| `sources.allow_https_batteries = false` | HTTPS Battery add `403` |
| `sources.allow_local_batteries = false` | Local/file Battery URLs `403` on HTTP |
| `sources.allow_private_https_batteries = false` | Private `token_ref` Battery add/sync `403` |
| `envs.http_manage = false` | Env create/update/delete/activate `403` |
| `envs.allow_secret_refs = false` | Env values as `secret://…` `403` |
| `runs.allow_env_selection = false` | Enqueue with explicit env `403` |
| `runs.allow_secret_fields = false` | Enqueue args with `secret://…` `403` |
| `scripts.max_content_bytes` / `tree_entry_limit` | Caps content/tree HTTP responses |
| `http.body_limit_bytes` | Caps JSON request bodies (and Axum body limit) |
| `secrets.metadata_endpoint = false` | `GET /v1/secrets` returns `404` |
| `auth.legacy_env_token = false` | Rejects `OMAKURE_API_TOKEN`; requires tokens file |
| `http.allow_non_loopback = true` | Same as CLI `--allow-non-loopback` |

Private HTTPS Batteries need scopes `batteries:write` (or add/sync) **and**
`credentials:use`, plus matching `--secret-ref` entries. Secrets metadata needs
`secrets:read-metadata` with `secrets.metadata_endpoint = true`.

Example read-only API:

```bash
omakure api --policy /etc/omakure/policy.toml --tokens-file /run/secrets/tokens.toml
```

## Topology choices

### API-only

HTTP management only — no in-process queue claiming or schedule scanner.

```bash
omakure token generate --id local --scope '*' --json
# write data.tokens_file_entry into secrets/tokens.toml
omakure api --bind 127.0.0.1:7878 --tokens-file secrets/tokens.toml
```

Equivalent engine form:

```bash
omakure engine --workers 0 --no-scheduler --tokens-file secrets/tokens.toml
```

Legacy single-token form (still supported):

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --capability scripts:read --capability runs:read \
  --capability runs:write --capability config:read
```

Use when another process owns workers/scheduler, or you only need read/write
management without draining the queue in this process.

### API + separate worker

Run the HTTP API (or API-only engine) in one process and drain the queue
elsewhere:

```bash
# Terminal A — API
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --capability scripts:read --capability runs:read --capability runs:write

# Terminal B — worker (same workspace / same SQLite)
omakure queue worker
```

Optional scheduler in a third process: `omakure serve --no-worker`.

All processes must share the **same workspace directory** (same
`.history/runs.sqlite`). Do not point them at different mounts.

### Engine (recommended single-process deploy)

`omakure engine` composes HTTP + optional embedded workers + the existing
schedule scanner, with coordinated SIGTERM shutdown (HTTP stop → stop
scheduling/claiming → drain workers).

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure engine --workers 1 \
  --capability scripts:read \
  --capability runs:read \
  --capability runs:write \
  --capability config:read
```

Container-friendly bind (inside the image default):

```bash
omakure engine --bind 0.0.0.0:7878 --allow-non-loopback --workers 1 \
  --capability scripts:read --capability runs:read --capability runs:write
```

Unauthenticated probes:

| Path | Auth | Purpose |
|------|------|---------|
| `GET /v1/health` | none | liveness |
| `GET /v1/ready` | none | readiness (optional worker/scheduler gates) |

Everything else requires Bearer auth. See `http-api.md`.

## Container image

Artifacts in the repo root:

| File | Role |
|------|------|
| `Dockerfile` | Multi-stage build → `omakure` binary; `ENTRYPOINT`/`CMD` run `engine` on `0.0.0.0:7878` with `--allow-non-loopback` |
| `.dockerignore` | Keeps build context lean |
| `compose.yaml` | Example: workspace volume, legacy token env, host bind `127.0.0.1:7878`, non-root guidance |

### Base image runtimes

The default runtime image installs **bash**, **git**, and **jq** (required for
supported scripts / `omakure doctor` required checks).

**Deferred (document only):** Python and PowerShell (`pwsh`) image variants.
Scripts that need those interpreters will fail until a variant image or host
install provides them — do not assume they are in the default image.

### Build

```bash
docker build -t omakure-engine:local .
```

### Compose example

```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
export OMAKURE_WORKSPACE="/path/to/your/omakure-scripts"
mkdir -p "$OMAKURE_WORKSPACE"
docker compose up --build
```

Compose publishes **`127.0.0.1:7878`** on the host. The container process still
listens on `0.0.0.0:7878` so the published port works. Prefer matching the
container `user` to the UID/GID that owns the mounted workspace if you hit
permission errors (image default is uid/gid `10001`).

## Volume layout

Mount one host directory as the workspace (`OMAKURE_SCRIPTS_DIR=/workspace`
in the image):

```
<workspace>/                 # mounted at /workspace
├── .omakure/                # envs, daemon artifacts, batteries
│   └── envs/
├── .history/                # runs.sqlite + search-index.sqlite
│   ├── runs.sqlite
│   └── search-index.sqlite
├── omakure.toml             # optional
└── <scripts…>
```

See `workspace.md` for the full contract.

## Single-host SQLite warning

Omakure's run queue and history are **SQLite files under `.history/`**.

- Run **one writer replica** per workspace volume.
- Do **not** put the workspace on NFS/CIFS/other network filesystems for
  multi-host sharing.
- Do **not** scale Compose `replicas` against the same volume.
- API + separate worker is fine **on the same host** sharing one local
  directory; it is not a distributed queue.

## Security checklist

- [ ] Prefer `--tokens-file` with Argon2id hashes; plaintext only from
      `omakure token generate` (prefix `omk_live_`), never committed.
- [ ] Per-token scopes are least-privilege; avoid `*` outside break-glass.
- [ ] Legacy `OMAKURE_API_TOKEN` (if used) is ≥32 random bytes; not a known
      weak default; gated with least-privilege `--capability`.
- [ ] Tokens live in env/secrets mounts — not in script env files or git.
- [ ] Host publish is loopback (`127.0.0.1:7878`) or behind a private network
      ACL / reverse proxy — never public internet.
- [ ] `--allow-non-loopback` only when the process cannot use loopback (e.g.
      container port publish).
- [ ] Container runs as non-root; volume ownership matches.
- [ ] Single replica / single host for the SQLite workspace.
- [ ] On Unix, plan for `SIGHUP` token reload after rotation.

Optional later: GHCR publish automation, audit-log isolation.

## Smoke checklist

Verify locally after packaging changes:

```bash
# 1. Build
docker build -t omakure-engine:local .

# 2. Prepare a disposable workspace + token
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
SMOKE_WS="$(mktemp -d)"
# optional: copy a tiny script tree into "$SMOKE_WS"

# 3. Run engine (API + one worker); map host loopback only.
# Use the host UID/GID so ensure_layout can write into the mounted workspace
# (image default USER is 10001 — fine when the volume is owned by that uid).
docker run --rm -d --name omakure-smoke \
  --user "$(id -u):$(id -g)" \
  -e OMAKURE_API_TOKEN \
  -e OMAKURE_SCRIPTS_DIR=/workspace \
  -v "$SMOKE_WS:/workspace" \
  -p 127.0.0.1:7878:7878 \
  omakure-engine:local \
  engine --bind 0.0.0.0:7878 --allow-non-loopback --workers 1 \
    --capability scripts:read --capability runs:read --capability runs:write \
    --capability config:read

# 4. Unauthenticated health
curl -sf http://127.0.0.1:7878/v1/health

# 5. Authenticated scripts / runs path (legacy token)
curl -sf -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/scripts
curl -sf -H "Authorization: Bearer $OMAKURE_API_TOKEN" \
  http://127.0.0.1:7878/v1/runs

# 6. Cleanup
docker stop omakure-smoke
rm -rf "$SMOKE_WS"
```

Packaging contract tests (file assertions, no Docker daemon required):

```bash
cargo test --test packaging_smoke
```

## Related docs

- `http-api.md` — route and auth contract
- `usage.md` — CLI including `engine` / `api`
- `workspace.md` — on-disk layout
- `scheduling.md` — standalone `omakure serve`
