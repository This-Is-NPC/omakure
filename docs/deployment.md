# Deployment guide

How to run Omakure's HTTP management surface on a single host: API-only,
API plus a separate worker, or the combined `omakure node serve` process —
including the repository's container image build.

This guide is for **trusted internal** deployments (loopback or private
network). It is not a public-internet threat model.

## Ownership

This guide owns deployment procedures: bind and topology choices, deploy
policy and load order, worker and scheduler lifecycle, startup/readiness,
containers, volumes, SQLite placement, certification, and smoke operation.
The [HTTP API contract](http-api.md) is the canonical source for routes,
authentication and scopes, JSON envelopes and errors, request limits,
CLI/HTTP adapter semantics, and request audit behavior.

## Auth boundary

Use the [HTTP API Authentication Contract](http-api.md#authentication-contract)
for the preferred multi-token file (`--tokens-file` /
`OMAKURE_TOKENS_FILE`), Argon2id hashes, per-token scopes,
`omakure token generate`, and SIGHUP reload semantics.

The legacy `OMAKURE_API_TOKEN` still works when no tokens file is set (internal
id `legacy`, scopes `*`, gated by process-wide `--capability`), unless deploy
policy sets `auth.legacy_env_token = false`. The policy and deployment
constraints for that mode are defined below.

## Deploy policy (`policy.toml`)

Deploy-only file via `--policy` / `OMAKURE_POLICY_FILE`. **Not** workspace
`omakure.toml`. Bad/missing required policy fails **before** binding a socket.

### Load order (api / node serve)

1. Built-in defaults (route groups allowed; node-service workers=1, scheduler on;
   `auth.legacy_env_token = true`).
2. Deploy `policy.toml` overlays `[http]` / `[node]` defaults and hard
   `[routes]` / `[auth]` gates.
3. Explicit CLI flags win when provided (`--bind`, `--workers`,
   `--scheduler` / `--no-scheduler`, readiness flags, `--tokens-file`,
    `--allow-non-loopback`, `--allow-non-loopback-direct`).
4. Workspace `omakure.toml` is never consulted for deploy policy.

Deploy route groups are checked **before** token scopes: a `*` token cannot
override `routes.writes = false` or `routes.battery = false`.

### Schema (v1)

Sections: `http`, `node`, `routes`, `sources`, `scripts`, `envs`, `runs`,
`secrets`, `auth`. Route groups and auth/node/http defaults apply globally;
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

[node]
workers = 2                    # used when --workers omitted
scheduler = true               # used when neither --scheduler nor --no-scheduler
readiness_requires_worker = true
readiness_requires_scheduler = true
readiness_requires_transport = true
allow_non_loopback_direct = false

[routes]
read = true
writes = true
battery = false
battery_install = false
run_enqueue = true
run_cancel = true
run_dead_letter = false
node = true
trust = true
enrollment = true
config = true
doctor = true
envs = true

[sources]
allow_https_batteries = true
allow_local_batteries = false
allow_private_https_batteries = true

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
max_concurrent_verifications = 2      # default; see Hard gates below
```

### Hard gates

| Setting | Effect |
|---------|--------|
| `routes.writes = false` | All write methods `403` (even `*` token) |
| `routes.node = false` | All `/v1/node/...` routes `403` (even `*` token) |
| `routes.trust = false` | Peer trust mutation routes `403`; peer reads remain governed by node/read gates |
| `routes.enrollment = false` | Enrollment write routes `403`; enrollment listing remains a read route |
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
| `auth.max_concurrent_verifications` | Caps in-flight Argon2id bearer verifications (default `2`, maximum `8`) |
| `http.allow_non_loopback = true` | Same as CLI `--allow-non-loopback` |
| `node.allow_non_loopback_direct = true` | Allows `network.direct_bind` or `--direct-bind` on a non-loopback address; independent of the HTTP bind policy and CLI `--allow-non-loopback` |

The direct transport listener is loopback-only unless the explicit direct
transport flag or policy setting is enabled. Static peers are validated for
unique node IDs and unique locators before the service starts. When
`readiness_requires_transport` is enabled, readiness requires every configured
static peer, not merely an equal-or-greater connection count.

Direct static-peer DNS uses the pure-Rust `hickory-resolver` async resolver with
the host system's DNS configuration (`/etc/resolv.conf` on Unix and the system
configuration on Windows). It is enabled only with its `system-config` and
`tokio` features, so it does not use libc `getaddrinfo`, an OS blocking worker,
or optional DNS-over-TLS/HTTPS/QUIC stacks. Each node service owns one bounded
resolver queue and runtime worker; service shutdown cancels queued and active
lookups, joins every resolver task, and then joins the worker. A lookup returns
all A and AAAA addresses, and the dialer tries them under the same absolute
10-second connect budget. This preserves Docker service-name resolution while
making timeout and restart lifetimes bounded and observable.

Private HTTPS Batteries need scopes `batteries:write` (or add/sync) **and**
`credentials:use`, plus matching `--secret-ref` entries. Secrets metadata needs
`secrets:read-metadata` with `secrets.metadata_endpoint = true`.

> **`auth.max_concurrent_verifications` tradeoff.** Each Argon2id bearer
> verify is memory-hard (~64 MiB); the bound keeps worst-case memory near
> `max_concurrent_verifications * ~64 MiB` and makes excess requests fail
> fast (`503 auth_busy`) instead of queuing unbounded. A tighter bound is
> also easier to exhaust: token ids are not secret (they're visible in the
> bearer string itself), so a party holding one valid id can occupy every
> permit with a stream of wrong-secret requests against that id, denying
> auth to all other tokens until a permit frees up. Raise the bound if your
> deployment has more legitimate concurrent auth traffic than headroom for
> this tradeoff, up to the maximum of `8` (~512 MiB). Larger values are
> rejected while loading policy so a configuration error cannot panic startup
> or permit an excessive authentication memory budget.

> **`--secret-ref '*'` and env vars.** The `*` secret-ref wildcard grants every
> file/provider ref but **does not** grant `secret://env/…` process-environment
> refs — those must be enumerated by exact name (`--secret-ref
> secret://env/GIT_TOKEN`). This prevents a token holder from reading arbitrary
> process env (e.g. `AWS_SECRET_ACCESS_KEY`) via a Battery `token_ref`. A bare
> `--secret-ref 'secret://env/*'` is ignored for the same reason. If you
> previously relied on `*` to resolve env secrets, list each `secret://env/NAME`
> explicitly.

### Network egress (SSRF containment)

Battery add/sync makes the node service issue outbound git requests. The node service
**rejects** hosts that are a literal private/loopback/link-local/metadata IP at
registration. At sync it resolves the host, rejects the operation if any answer
is private, and pins Git/curl to one verified public answer with
`http.curloptResolve`. Redirects and inherited HTTP proxies are disabled, so a
remote cannot redirect credentials or make Git resolve a second host. Failure
to resolve and pin fails closed.

Keep defense in depth by running the node service behind a **network egress policy** that
denies traffic to RFC1918 / loopback / link-local / cloud-metadata ranges — a
Kubernetes `NetworkPolicy`/egress rule, a firewall, or a locked-down network
namespace. Battery sync intentionally ignores process proxy variables; deploy
the egress control transparently rather than through `HTTP_PROXY`/`HTTPS_PROXY`.

Example read-only API:

```bash
omakure api --policy /etc/omakure/policy.toml --tokens-file /run/secrets/tokens.toml
```

## Topology choices

### API invocation forms

```bash
# Preferred: multi-token file (per-token scopes; --capability ignored)
omakure api --bind 127.0.0.1:7878 --tokens-file /run/secrets/omakure_tokens.toml

# Legacy single-token mode (set the required token before either alternative)
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"

# Loopback alternative
omakure api --bind 127.0.0.1:7878 \
  --capability config:read \
  --capability scripts:read \
  --capability runs:read \
  --capability runs:write \
  --capability env:read

# Non-loopback alternative
omakure api --bind 0.0.0.0:7878 --allow-non-loopback \
  --capability config:read \
  --capability scripts:read \
  --capability runs:read \
  --capability env:read \
  --capability env:write
```

See the [HTTP API contract's scope and legacy capability matrix](http-api.md#scope-matching-and-legacy-capabilities)
for accepted capability names, scope matching, and secret-ref requirements.

### Node service lifecycle

`omakure node serve` runs the same HTTP surface as `omakure api`, plus
optional in-process queue workers and the existing schedule scanner, with
coordinated SIGTERM shutdown: stop HTTP acceptance, stop scheduling and
claiming, then drain workers.

Built-in defaults are one worker and the scheduler enabled. `--workers 0`
selects API-only operation; `--scheduler` / `--no-scheduler` explicitly
enables or disables the scanner. The readiness flags are opt-in:

- `--readiness-requires-worker` makes `/v1/ready` fail if configured workers
  are not alive.
- `--readiness-requires-scheduler` makes `/v1/ready` fail if the scheduler is
  enabled but not alive.
- `--readiness-requires-transport` makes `/v1/ready` fail while any configured
  static peer is disconnected. The equivalent deploy-policy setting is
  `node.readiness_requires_transport`; when either is enabled, every
  configured static peer must be connected.

The default HTTP bind is `127.0.0.1:7878`, and loopback needs no extra flag.
Non-loopback binds, including `0.0.0.0` and `::`, require
`--allow-non-loopback` **or** deploy policy `[http] allow_non_loopback = true`;
the service must reject the configuration before it listens when neither
guard is satisfied. `--tokens-file` / `OMAKURE_TOKENS_FILE` selects the
multi-token authentication mode described in the
[HTTP API contract](http-api.md#authentication-contract).

### API-only

HTTP management only — no in-process queue claiming or schedule scanner.

```bash
omakure token generate --id local --scope '*' --json
# write data.tokens_file_entry into secrets/tokens.toml
omakure api --bind 127.0.0.1:7878 --tokens-file secrets/tokens.toml
```

Equivalent node-service form:

```bash
omakure node serve --workers 0 --no-scheduler --tokens-file secrets/tokens.toml
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

Run the HTTP API (or API-only node service) in one process and drain the queue
elsewhere:

```bash
# Terminal A — API
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure api --capability scripts:read --capability runs:read --capability runs:write

# Terminal B — worker (same workspace / same SQLite)
omakure queue worker
```

Optional scheduler in a third process: `omakure serve --no-worker`.

All processes must share the **same workspace directory on one host** (same
local `.history/runs.sqlite`). Do not point them at different mounts, network
filesystems, or another host: the queue is not distributed.

### Node service (recommended single-process topology)


```bash
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
omakure node serve --workers 1 \
  --capability scripts:read \
  --capability runs:read \
  --capability runs:write \
  --capability config:read
```

Container-friendly bind (inside the image default):

```bash
omakure node serve --bind 0.0.0.0:7878 --allow-non-loopback --workers 1 \
  --capability scripts:read --capability runs:read --capability runs:write
```

Two rules cost a real debugging session on a provisioned machine; both are
enforced, neither is guessable from the config file alone.

**`api.bind` in `node.toml` must stay loopback.** `NodeConfig` rejects a
non-loopback configured value before API boot. To bind `node serve` wider, pass
an explicit non-loopback `--bind` and authorize it with
`--allow-non-loopback` or deploy policy `[http] allow_non_loopback = true`;
the service refuses the bind before listening when neither authorization is
present.

**Enrolment is time-bound, so fix the clock first.** A signed bundle carries a
validity window. A machine whose clock is off by an hour will refuse a
perfectly good bundle with `enrollment_expired`, which reads like a stale
bundle and is not. Confirm `timedatectl` reports a synchronised clock on the
joining machine before issuing anything.

For unauthenticated health/readiness behavior and all other route
authentication requirements, see the [HTTP API endpoint contract](http-api.md#endpoint-contract).

## Container image

Artifacts in the repo root:

| File | Role |
|------|------|
| `Dockerfile` | Multi-stage build → `omakure` binary; `ENTRYPOINT`/`CMD` run `node serve` on `0.0.0.0:7878` with `--allow-non-loopback` |
| `.dockerignore` | Keeps build context lean |
| `compose.yaml` | Example: workspace and tokens-file volumes, host bind `127.0.0.1:7878`, fixed uid/gid `10001` |
| `ci/compose/compose.transport-certification.e2e.yaml` | Isolated four-service Linux certification topology; not a production fleet deployment |
| `scripts/tasks/cert/transport` | Bounded canonical certification runner used locally and in Linux CI |
| `ci/compose/compose.health-plane-certification.e2e.yaml` | Isolated four-node Health Plane certification topology; not a production fleet deployment |
| `scripts/tasks/cert/health` | Bounded canonical Health Plane gate used locally and in Linux CI |
| `scripts/tasks/cert/health-cleanup` | Verifies Health Plane certification cleanup after induced failure and after interrupt |

The `enrollment-target` and `enrollment-candidate` services in `compose.yaml`
are the Docker discovery E2E fixture, not a production topology. Each service
healthcheck validates the semantic `GET /v1/ready` response, and
`enrollment-candidate` is ordered after a healthy target. Both services pass
`--readiness-requires-worker`, so a process with a dead in-process worker never
reports ready to Compose.

### Base image runtimes

The default runtime image installs **bash**, **git**, **jq**, and **curl** (required for
supported scripts / `omakure doctor` required checks).

**Not in the default image:** Python and PowerShell (`pwsh`). Scripts that need
those interpreters fail unless a variant image or a host install provides them.

### Build

```bash
docker build -t omakure-node:local .
```

### Compose example

```bash
mkdir -p secrets
omakure token generate --id ops --scope scripts:read --scope runs:read \
  --scope runs:write --scope config:read \
  --append secrets/omakure_tokens.toml --confirmed
# Save the generated plaintext token shown once on stdout in your secret store.
export OMAKURE_WORKSPACE="/path/to/your/omakure-scripts"
mkdir -p "$OMAKURE_WORKSPACE"
# Compose runs as the fixed image principal 10001:10001. Prepare both bind
# mounts; do not substitute the invoking user's UID/GID.
export OMAKURE_NODE_STATE="${OMAKURE_NODE_STATE:-$(pwd)/node-state}"
install -d -o 10001 -g 10001 -m 0700 "$OMAKURE_NODE_STATE"
chown 10001:10001 "$OMAKURE_WORKSPACE"
docker compose up --build
```

`compose.yaml` uses tokens-file auth by default and does not require
`OMAKURE_API_TOKEN`. To use the legacy single-token mode instead, remove or
comment the `OMAKURE_TOKENS_FILE` environment entry and tokens-file volume,
uncomment the `OMAKURE_API_TOKEN` entry, then export a token of at least 32
random bytes before starting Compose.

Compose publishes **`127.0.0.1:7878`** on the host. The container process still
listens on `0.0.0.0:7878` so the published port works. The image always runs as
uid/gid `10001:10001`; host bind mounts must be prepared for that principal.
The image-owned `/etc/omakure/node.toml` remains `root:omakure` mode `0640`.
`0640` is the broadest mode the node accepts, not the only one: any
stricter mode the service can still read, such as `0600`, is accepted too.
A broader one is refused, and the refusal names the file, the mode it
found, and the `chmod` that fixes it. The private key material under
`/var/lib/omakure` is held to `0600` by the same rule, so no group or
other bit is ever accepted there.

## Transport certification topology

The canonical Linux gate is deliberately separate from the example deployment:

```bash
mise run cert:transport
```

It builds the current image and starts `cert-a`, `cert-b`, `cert-c`, and an
untrusted `cert-adversary` on an isolated Compose network. Each service has
separate state, config, token, runtime, and workspace volumes. The runner uses
production direct listeners plus authenticated management operations only for
observation and lifecycle setup. The production-listener integration cases cover
malformed, oversized, and downgraded frames; expired certificates (1008), forged
certificates and envelopes (1004), identity-mismatched certificates and
wrong-target probes (1005), exact encrypted-byte replay (1009), untrusted
traffic, static-peer validation, partition/reconnect, revocation, and identity
reset/replacement. Each rejection checks the matching durable redacted audit and
full registry-state snapshot. It always removes the project, containers,
network, and volumes on exit. The topology is for a bounded certification gate
only and must not be treated as a general fleet launcher. Cleanup verification
fails closed: an error while inspecting containers, networks, or volumes is a
failed gate, not evidence that cleanup succeeded.

## Health Plane certification topology

The Health Plane has its own bounded Linux gate, separate again from both the
example deployment and the transport gate:

```bash
mise run cert:health
```

It builds the current image and starts `hp-node-1` through `hp-node-4` on one
dedicated Compose network. Each service owns separate identity/trust (`state`),
config, workspace, token, and runtime volumes. Fleet roles are assigned at
runtime from the freshly generated canonical node IDs, because the shipped
transport resolves dial ownership deterministically from them: ranked ascending,
the two lowest IDs become the Performers, the third becomes the untrusted
adversary, and the highest becomes the Conductor.

Management HTTP binds `127.0.0.1:7878` inside every container and is never
published, so it is structurally incapable of carrying node-to-node data; the
gate asserts that peer-to-peer HTTP is unreachable and that only the direct
transport port `7879` is published. Every Health Plane message therefore crosses
production Noise.

The gate proves Profile, Pulse, fleet aggregation through both the CLI and HTTP
read surfaces, all three Signal kinds, one idempotent `run-completed` Signal
from a real manual `omakure run`, `online` → `stale` → recovery across a real
network partition, restart persistence, revocation exclusion plus retention
purge, identity replacement, and frozen attempt exhaustion over one continuously
connected session. Adversarial cases are injected over real production Noise
sessions by `tests/docker_health_plane_adversary.rs`, which dials into the
published transport listeners from the host. The attempt-exhaustion harness is
the one case where the Performer must be the initiator, so it runs as the
`hp-harness` container on the same dedicated network rather than as a host
process; no phase requires container-to-host reachability, and the gate is
therefore unaffected by a default-deny host firewall. It always removes the
project, containers, network, and volumes on exit, and fails if any survive.
Cleanup verification also fails closed when inspection errors occur, rather than
treating the resources as absent. The topology is a bounded certification gate
only and must not be treated as a general fleet launcher.

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
- [ ] Use `SIGHUP` token reload after rotation on Unix.
- [ ] Keep `OMAKURE_API_TOKEN` out of scripts and script environment files;
      rotate management tokens like any other secret.
- [ ] Keep request bodies at the 1 MiB v1 limit and leave browser CORS
      disabled.
- [ ] Treat Battery cache content as untrusted: HTTP may list, sync, inspect,
      and install through Battery operations, but must not execute scripts
      directly from `.omakure/batteries/cache/`.
- [ ] Register HTTP Batteries only from `https://` sources; use the local CLI
      for local development repositories.

## Smoke checklist

Verify locally after packaging changes:

```bash
# 1. Build
docker build -t omakure-node:local .

# 2. Prepare a disposable workspace + token
export OMAKURE_API_TOKEN="$(openssl rand -hex 32)"
SMOKE_WS="$(mktemp -d)"
# optional: copy a tiny script tree into "$SMOKE_WS"

# 3. Run node service (API + one worker); map host loopback only.
# Prepare fixed image-principal ownership; do not pass an arbitrary --user.
sudo install -d -o 10001 -g 10001 -m 0750 "$SMOKE_WS"
sudo install -d -o 10001 -g 10001 -m 0700 "${SMOKE_WS}-node-state"
docker run --rm -d --name omakure-smoke \
  -e OMAKURE_API_TOKEN \
  -e OMAKURE_SCRIPTS_DIR=/workspace \
  -v "$SMOKE_WS:/workspace" \
  -v "${SMOKE_WS}-node-state:/var/lib/omakure" \
  -p 127.0.0.1:7878:7878 \
  omakure-node:local \
  node serve --bind 0.0.0.0:7878 --allow-non-loopback --workers 1 \
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
sudo rm -rf "${SMOKE_WS}-node-state"
```

Packaging contract tests (file assertions, no Docker daemon required):

```bash
cargo test --test packaging_smoke
```

## Related docs

- `http-api.md` — route and auth contract
- `usage.md` — CLI including `node serve` / `api`
- `workspace.md` — on-disk layout
- `scheduling.md` — standalone `omakure serve`
