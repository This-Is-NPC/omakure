# Rebuild Omakure — Canonical Product Direction and Node Contract

> Status: canonical future direction and frozen contracts. Future features below
> are not shipped unless the implemented baseline says otherwise.
> Branch: `rebuilding-omakure-for-omarchy`

## One-line vision

Omakure becomes an **Omarchy-first, decentralized, cross-platform MDM** built
as a peer-to-peer network of automation nodes: a headless (CLI + HTTP) tool
where any node can be delegated as the manager of other nodes, discovers
peers, reports health, runs authorized work remotely, and executes Lua (and
other) scripts. The node core is generic and runs on Linux, macOS, and Windows;
Omarchy is the reference platform with the richest installation, profile, and
desktop integration, never a runtime dependency.

## Non-negotiable constraints

These are locked product decisions, not implementation choices:

- **100% TUI removal.** Omakure itself is CLI + HTTP only. The terminal UI, its
  widgets, snapshots, and the entire theme subsystem are gone. Optional
  Omarchy Shell/Menu integrations consume the same CLI/HTTP contracts and do
  not introduce a second Omakure UI or duplicate business logic.
- **Self-contained control plane, lightest-practical bundle.** The binary
  embeds the node control plane and Lua runtime (via `mlua` with the `vendored`
  feature), so `.lua` automation needs no system Lua. Bash, PowerShell, and
  Python remain optional script types and use runtimes available on the host.
  Removing ratatui/crossterm/rattles is the main bundle win; binary size and
  linkage are measured release properties, not absolute portability claims.
- **Cross-platform MVP.** The portable node core and every mesh protocol work
  on generic Linux, macOS, and Windows, as well as Omarchy. Platform-specific
  capabilities report `unsupported` rather than breaking the node. Omarchy is
  the reference platform and first-class product experience, never a runtime
  requirement for the portable core.

## What Omarchy-first means

- Omarchy is the reference platform for product documentation, demos, release
  validation, and the default fleet journey.
- Unattended Omarchy installation can provision public node configuration,
  signed enrollment authority data, a bootstrap token, manager public keys,
  and policy. The target service always generates its private keypair and node
  identity on first start, then uses that data to join its fleet without a
  manual post-boot step.
- Omarchy nodes expose the richest Profile, including Omarchy version/channel
  and relevant system capabilities.
- Optional Omarchy hooks, notifications, and Shell/Menu integrations surface
  node and fleet state through the native Omarchy experience.
- The wire protocol, identity format, `node.toml`, CLI, and HTTP contracts never
  depend on Omarchy and remain identical on other supported platforms.

## What Omakure becomes

A node is a single Omakure installation. Nodes form a peer-to-peer network with
no mandatory hosted server. Any node can be promoted to **Conductor** (manager)
of other nodes, which then become its **Performers** (managed nodes). The
Conductor is the delegated control point where an operator sees that fleet.
"Mesh" describes direct trusted node relationships and discovery; it does not
promise distributed consensus, packet routing, or mandatory all-to-all links.

### Vocabulary (product terms)

| Term | Meaning |
|------|---------|
| **Node** | One Omakure installation on one machine (any OS/distro) |
| **Mesh** | The peer-to-peer network of nodes (no fixed server) |
| **Conductor** | A node delegated to manage other nodes (the manager / control plane) |
| **Performer** | A node managed by a Conductor (the managed node) |
| **Pulse** | Periodic health/heartbeat from a Performer |
| **Profile** | Static node facts: OS, architecture, runtimes, capabilities |
| **Signal** | A notable event emitted by a node (run finished, enrolled, revoked) |
| **Cue** | A signed unit of remote work sent to a Performer |
| **Ensemble** | A named group of nodes |

## Feature set

### 1. Node identity & trust
- Every node has a cryptographic identity (key pair) and a stable node id.
- Node-to-node communication is encrypted, mutually authenticated, and signed;
  unsigned, expired, duplicated, or replayed messages are rejected.
- One credential per Performer — no fleet-wide shared token.
- Identity is cross-distro: no reliance on OS-specific stores.
- Identity rotation and revocation are first-class operations; a node validates
  current trust when reconnecting after being offline.

### 2. Discovery
- Nodes announce themselves on the local network and discover peers
  automatically (LAN discovery).
- Static seed peers can be declared for environments where broadcast discovery
  is unavailable.
- A Performer can be enrolled into a Conductor via a walk-up code + key exchange
  with authenticated key confirmation (secure pairing, no pre-shared fleet
  secret).
- A signed enrollment bundle plus public bootstrap data support unattended
  autojoin; the target service generates its identity on first start and trust
  is persisted only after that authenticated exchange. Walk-up codes are the
  manual enrollment path, not a required boot-time step.

### 3. Health & status (the control plane)
- Each Performer emits a **Pulse** (liveness + resource/runner health) and
  **Profile** (facts, including OS/distro) to its Conductor.
- The Conductor aggregates Pulses and presents a single status view of all
  managed nodes: who is online, health state, last run, capabilities.
- **Signals** surface notable events across the fleet.
- This is the "central node where I see status of managed nodes."

### 4. Delegation & management
- Any node can be **delegated** as Conductor of another node (and revoked).
- A Conductor can **run scripts remotely** on its Performers (authorized Cues)
  and see results. Every Cue has a stable id, expiration, idempotency semantics,
  and an auditable outcome.
- Management is opt-in per capability: a Performer decides whether it allows
  remote-run, baseline push, etc. Authorization can restrict operation type,
  script, arguments, secret access, timeout, concurrency, and privilege level.

### 5. Script execution (self-contained)
- Omakure runs automation scripts of multiple kinds. **Lua is a first-class
  script type**, executed by the embedded Lua runtime — no system Lua required.
- Bash, PowerShell, and Python remain supported (resolved on the host where the
  script runs).
- The same schema-driven input contract applies regardless of script language;
  CLI and HTTP clients render or supply those inputs without an Omakure form UI.

### 6. MDM-style capabilities (mirrors commercial MDM)
Aligned with what market MDM tools (Omadium, Hexnode, Fleet, ManageEngine)
provide — scoped to operational management, **never surveillance**:

- **Remote execution** — trigger scripts/runs on a Performer from the Conductor.
- **Deployment baseline** — push environments, schedules, and scripts from
  Conductor to Performers so machines stay in sync. Baselines are signed,
  versioned, previewable, auditable, and recoverable when application fails.
- **Lost-device basics** — revoke a node's access / disable its runner if a
  machine goes missing; verify disk encryption status.
- **Status & health** — as above.
- **Explicitly excluded:** no screen recording, keylogging, browsing/location
  tracking, or productivity scoring. Omakure collects only what is needed to
  manage the runner.

### 7. Testable & installable (any host)
- Everything is headless and deterministic: node commands accept a config file
  and ephemeral paths/ports, so they run in CI and tests without a display.
- A single **install-time config** (`node.toml`) declares public node settings,
  discovery, and capability policy. It never contains a private keypair or
  node identity; signed enrollment authority data supplies any bootstrap trust.
- Bootstrap is cross-platform: the node daemon auto-starts via systemd on
  Linux, launchd on macOS, and a Windows Service or Task Scheduler integration
  on Windows.
- On Omarchy specifically, unattended installation can additionally provision
  `node.toml`, signed enrollment authority data, a bootstrap token, manager
  public keys, and policy. The fresh machine generates its keypair and node
  identity on the target service's first start. Manual pairing and the same
  public-data provisioning contract remain available on every supported
  platform.
- Fleet simulation covers heterogeneous Omarchy, generic Linux, macOS, and
  Windows nodes, including offline reconnect, replay rejection, revocation,
  and duplicate Cue delivery.

## Direction by phase (feature view, not tasks)

1. **Strip to CLI + HTTP** — remove TUI and theme subsystem while preserving
   schemas, JSON envelopes, queue states, history, traces, secret redaction,
   schedules, and Batteries.
2. **Portable node core — complete** — target-generated identity, `node.toml`,
   isolated peer registry, node service lifecycle, and platform adapters for
   Linux, macOS, and Windows.
3. **Secure transport & pairing — complete for the bounded direct foundation** —
   encrypted authenticated channels, static peers, LAN discovery, manual
   enrollment, unattended signed-bundle enrollment, revocation, replay
   protection, and reset/replacement recovery. Nostr fallback and later fleet
   planes are not included in this phase.
4. **Health plane — complete for the minimal bounded plane** — versioned
   Profile, Pulse, and a closed three-kind Signal feed (`enrolled`, `revoked`,
   `run-completed`) carried inside the frozen direct envelope, bounded current
   health state with latest-state-only retention, and a Conductor fleet-status
   projection read through `omakure node health` / `GET /v1/node/health` and
   `omakure node signals` / `GET /v1/node/signals`. Contracts and every
   quantitative bound are frozen in `.docs/health-plane-contract.md`. Baselines,
   dashboards, alert engines, arbitrary metrics, long-term telemetry, and remote
   Cues are not included in this phase.
5. **Script execution** — Lua as an embedded first-class script kind; retain
   Bash, PowerShell, and Python through host runtimes.
6. **Remote management — complete for authorized Cues** — a trusted Conductor
   asks a Performer to run a script that Performer already declared, behind five
   fail-closed gates read only from the receiver's own registry and config. A
   Cue names a script and never carries one, runs with deny-all secret access,
   executes at most once, and reports its outcome on the existing
   `run-completed` Signal. Broader delegation and campaign fan-out remain open.
7. **Omarchy-first experience** — unattended public-data provisioning, rich
   Profile, hooks, notifications, and optional Shell/Menu integrations.
8. **MDM basics** — signed/versioned baseline push, drift status, rollback, and
   lost-device revoke.
9. **Install automation** — systemd, launchd, and Windows daemon bootstrap.
10. **Test & doc** — heterogeneous fleet simulation, install-config tests,
    security failure tests, platform release validation, and product docs.

## Success looks like

- `omakure` with no subcommand no longer opens a UI — it is a daemon/CLI.
- The control plane is self-contained and deliberately small; it runs `.lua`
  scripts with no system Lua installed and reports optional host runtimes.
- A heterogeneous fleet can use an Omarchy Conductor with Omarchy, generic
  Linux, macOS, and Windows Performers over the same protocol and config model.
- On my LAN, a new Omakure node shows up automatically; I delegate one as
  Conductor and see every node's health in one place.
- I can run a script on any node from the Conductor, push a baseline, and revoke
  a lost machine — all from CLI/HTTP.
- An unattended machine joins because its public install config and signed
  enrollment data are present: the target service generates its identity on
  first start and completes enrollment without a manual step after boot. An
  unprovisioned machine can join through secure walk-up pairing.
- Omarchy provides the most integrated install and fleet experience without
  changing the portable node behavior available on other platforms.

## Appendix — direction context vs MDM market

For product direction only (not a commitment to copy). From omadium.app and
common MDM tools:

| Omadium / MDM capability | In Omakure? | Notes |
|---|---|---|
| IdP login (Okta/Entra/Google) | Out of scope | OS-level login, not node management. Omakure uses key-based node identity. |
| Remote help / support access | Partial | Remote script execution (Cue) + capability allow-list; not interactive screen-share. |
| Deployment baseline | Yes | Baseline push (environments/schedules/scripts) + Pulse "in sync". |
| Lost-device basics | Yes | `revoke` Cue + Profile reports disk encryption. |
| No surveillance | Yes | Same principle: only operational health. |

**Positioning:** Omakure is an open-source, self-hosted, **decentralized** MDM
for secure, script-native fleet automation. It is Omarchy-first in product
experience and generic across Linux, macOS, and Windows at the node/protocol
level. It is not a full device MDM: it does not own OS login or enforce disk
encryption itself; it reports relevant posture, manages the runner, distributes
approved baselines, and revokes node trust.

## Implemented v0.3.0 baseline

This section is an inventory of what is shipped today. It is deliberately
separate from the future node contract below; it must not be read as evidence
that the later health, remote-management, MDM, or Lua features have been
implemented.

- The package is `omakure 0.3.0`, a headless Rust CLI and authenticated HTTP
  management API.
- The completed portable node command is `omakure node serve`; it owns the
  machine identity and isolated trust registry while composing the authenticated
  HTTP API, optional queue workers, and scheduler through shared lifecycle code.
  Its readiness gate is not reached when node state is corrupt, insecure,
  mismatched, unsupported, or unwritable.
- Script state is workspace-owned under `.omakure/` and `.history/`; the future
  node state defined here is not implemented and must never be inferred from a
  script workspace.
- Shipped script kinds are Bash, PowerShell, and Python. The portable node
  foundation includes identity, `node.toml`, isolated `node.sqlite`, explicit
  trust management, service lifecycle, reset, and platform path validation.
- Direct transport, trust-neutral discovery, manual enrollment, signed-bundle
  enrollment, static-peer lifecycle, revocation, replay protection, and
  reset/replacement recovery are shipped, as are Profiles, Pulses, the closed
  Signal feed, and authorized remote Cues. Nostr, campaigns, and MDM remain
  future work.
- The production identity implementation is the reviewed RustCrypto `k256`
  BIP-340 adapter documented below; no second identity implementation exists.

## Frozen node boundary

The service command is `omakure node serve`. It is the product command for the
machine node. The service is one machine/service identity, independent of any script
workspace. A node may expose the existing script runner later, but a workspace
must never own, select, copy, or silently reset the node identity or trust DB.

### Ownership and default paths

The installer creates the service account, directories, and restrictive ACLs.
Paths are absolute and are not subject to workspace or current-directory
resolution. A missing parent, symlink, wrong owner, group/world write access, or
unexpected file type is an initialization error.

| Platform | Service | Config | State | Required ownership and permissions |
|---|---|---|---|---|
| Linux | `omakure-node.service`, user/group `omakure:omakure` | `/etc/omakure/node.toml` | `/var/lib/omakure/` | config `root:omakure` `0640`; state and `node.sqlite` `omakure:omakure` `0700`/`0600`; identity `0600` |
| macOS | `/Library/LaunchDaemons/com.omakure.node.plist`, user/group `_omakure:_omakure` | `/Library/Application Support/Omakure/node.toml` | `/Library/Application Support/Omakure/` | config `root:_omakure` `0640`; state and database `_omakure:_omakure` `0700`/`0600`; identity `0600` |
| Windows | service name `OmakureNode`, account `NT AUTHORITY\LocalService` | `%ProgramData%\Omakure\node.toml` | `%ProgramData%\Omakure\` | directory and files use a DACL granting `LocalService` and `SYSTEM` access only; identity and database are not inheritable by users |

The service state directory contains exactly the normalized scalar file
`identity.key`, `node.sqlite`, and `node.sqlite-wal`/`node.sqlite-shm` while
SQLite is open. `identity.pub` is forbidden; there is no persisted public-key
companion file. The config is public policy and contains no private key,
secret value, enrollment token, or trust database. The workspace may be on a
different volume or absent entirely.

For deterministic integration tests only, `--node-state-dir PATH` and
`--node-config PATH` are explicit overrides. The test harness must pass both
absolute paths under a freshly created private directory and set
`OMAKURE_NODE_TEST_MODE=1`; installed service units never set that variable.
Ambient workspace variables and the current directory are never fallbacks for
node paths. Override precedence is CLI flag, then the corresponding test-mode
environment variable (`OMAKURE_NODE_STATE_DIR` or `OMAKURE_NODE_CONFIG`), then
the platform default. A production invocation rejects test-mode variables.

### `node.toml` version 1

The parser accepts only the following fields. `version`, every table, and every
listed key are required; the values shown are the complete v1 safe baseline,
not implicit behavior.
Unknown keys, duplicate keys, missing keys, invalid types, invalid enum values,
unknown versions, and trailing data are errors. There is no best-effort parse
and no automatic downgrade. A future version must use a new parser and an
explicit migration.

```toml
version = 1

[node]
display_name = ""                 # public label; empty means no label

[api]
bind = "127.0.0.1:7878"           # loopback only unless a future policy opts in

[network]
mode = "direct"                   # direct | direct-with-nostr-fallback | nostr
relays = []                        # wss:// URLs; used only by non-direct modes
static_peers = []                 # strings of the form node_id@host:port
max_message_bytes = 1048576

[trust]
enrollment = "disabled"            # disabled | manual | signed-bundle
allow_remote_cues = false
allow_baseline_push = false

[organization]
id = ""                            # empty means unmanaged
discovery_secret_ref = ""          # empty or secret:// reference, never a value
```

`display_name`, organization id, relay URLs, static peers, bind address, and
capability flags are public configuration. `discovery_secret_ref` may name a
configured secret provider using the `secret://provider/name` grammar; the
resolved value is process-only and is never written to `node.toml`, SQLite,
logs, Cues, relay events, notifications, or HTTP output. Private keys and
manager trust are not configuration fields: they live in `identity.key` and
`node.sqlite`, respectively. `enrollment = "disabled"`, both false capability
flags, loopback bind, and direct mode are explicit safe defaults.

The config is validated before identity creation. A valid config with no
organization and disabled enrollment can start as an unmanaged node, but it
cannot discover, enroll, accept a manager, or execute a remote Cue.

### Identity and node ID

The implementation uses RustCrypto `k256` 0.14.0, with only the features
needed for secp256k1 scalar arithmetic, x-only public keys, BIP-340 Schnorr
signatures, and OS entropy. It provides the sole production identity
implementation; no second identity implementation is added.

- A generated or imported private scalar is exactly 32 big-endian bytes in
  `[1, n-1]`. Let `Q = dG`; if `Q.y` is odd, replace `d` with `n-d`. This
  normalization is mandatory before any persistence, rotation, recovery, or
  signing, and the resulting scalar is the only identity material stored in
  `identity.key`. It is never stored as a mnemonic, PEM, environment value, or
  JSON string.
- Production generation uses the operating system CSPRNG through the crate's
  `getrandom` path. Deterministic keys are test fixtures only and are rejected
  by production initialization. An imported scalar is normalized before its
  atomic write; an existing persisted scalar that is not normalized fails
  closed rather than being silently rewritten.
- The canonical public identity is the 32-byte BIP-340 x-only public key, the
  x coordinate of the even-Y point after normalization. Its only wire/text form
  is 64 lowercase hexadecimal characters with no `02`/`03` prefix and no
  `0x`. There is no `identity.pub` file; public identity is derived in memory
  or carried in protocol fields.
- BIP-340 Schnorr is the sole node/application signing algorithm. No alternate
  signing algorithm or parity-sensitive alias is part of the contract. The same
  normalized scalar and x-only public key are used for direct envelopes and
  optional Nostr transport. Direct Noise transport uses a separate service-owned
  static X25519 key only; that key is bound to this identity by the signed
  transport certificate and never defines node identity or trust.
- `node_id` version 1 is the ASCII string `omk1_` followed by lowercase hex of
  `SHA-256(b"omakure/node-id/v1\\0" || x_only_public_key)`. The NUL is part of
  the hashed input. The version marker is both the `omk1_` prefix and the
  domain separator; changing either creates a new node-id derivation version.
  Scalars `d` and `n-d` therefore produce the same normalized scalar, public
  key, and node ID.
- A node ID is an identifier, not proof of trust. Knowing an ID, relay URL,
  organization ID, or discovery secret never authorizes a manager or Cue.

The exact vectors used by later tests are in
`tests/fixtures/node_identity_vectors.toml`. They contain no production secret;
the scalar values are intentionally public test inputs. The vectors include
both the input scalar and the normalized scalar so parity normalization and the
`d == n-d` equivalence are directly testable. They were independently checked
against a secp256k1 oracle and SHA-256 using OpenSSL 3.6.3 and `sha256sum`.

### Signing prehash contracts

Direct and Nostr signatures use the same normalized BIP-340 key, but their
message construction is not interchangeable:

- A direct envelope excludes its signature field and is serialized as strict
  RFC 8785 JSON Canonicalization Scheme UTF-8 bytes: sorted object keys, no
  insignificant whitespace, no duplicate keys, integer-only numbers, and
  preserved array order. Its signing message is
  `SHA-256(ASCII("omakure/direct-envelope/v1"), NUL byte, canonical bytes)`.
  BIP-340 signs that resulting 32-byte message; no other application hash or
  domain is added.
- A Nostr event uses NIP-01 serialization of
  `[0, pubkey, created_at, kind, tags, content]`, where `pubkey` is the 64
  lowercase x-only key. The event ID is the SHA-256 of those serialized bytes.
  BIP-340 signs the 32-byte event ID directly, without an additional
  application prehash or double hashing. NIP-01 event IDs, signatures, and
  relay fields remain distinct from the direct envelope contract.

### Crypto feasibility record

The feasibility check was performed on this branch with `rustc 1.97.1` and
`cargo 1.97.1`; the crate advertises Rust 1.85+ and therefore fits the
repository toolchain. `k256` is pure Rust, dual licensed Apache-2.0/MIT, and
does not link OpenSSL or a platform C library. Its public documentation lists
  secp256k1 Schnorr/BIP-340, x-only public-key operations, and constant-time
  scalar multiplication. The repository's supported targets are
Linux, macOS, and Windows; `k256` has no OS-specific crypto code and its
`getrandom` backend supplies platform entropy on all three. The installed CI
target here is `x86_64-unknown-linux-gnu`; cross-target builds remain a release
gate before the dependency is introduced.

The choice is preferable to a C-linked library because it preserves the
binary-only packaging model and avoids a system crypto ABI. `ring` does not
provide the required secp256k1 identity API. OpenSSL would add platform and
runtime linkage obligations. The upstream repository records an NCC Group
audit and corrected findings, while retaining its explicit "use at your own
risk" notice; the implementation task must pin the exact release, review the
security notes, enable no unnecessary algorithms, and run upstream and local
vector tests. References: `https://crates.io/crates/k256`,
`https://docs.rs/k256/`, and
`https://github.com/RustCrypto/elliptic-curves/tree/master/k256`.

The direct authenticated channel and enrollment contract is recorded in
`.docs/direct-transport-contract.md`. The owner approved the standard pure-Rust
Noise `XX_25519_ChaChaPoly_SHA256` construction through `snow` 0.10.0, with a
service-owned static X25519 key bound to the canonical normalized
secp256k1/BIP-340 identity by a signed certificate. The exact direct-channel
wire format, handshake, session state machine, enrollment format, limits, and
schema migration boundary are frozen there. No production transport is
implemented; dependent tasks must implement that contract without inventing
alternate bytes or trust paths.

### Initialization, lifecycle, and recovery

`node serve` follows these rules in order:

1. Resolve and validate the config and both path boundaries.
2. Open the state directory without following symlinks and verify owner,
   permissions, and file types. A missing directory may be created atomically
   only with the platform defaults or explicit test overrides.
3. Before generating anything, inspect any existing `node.sqlite` and reject an
   `identity.pub` file as an unsupported extra. If `node.sqlite` exists while
   `identity.key` is absent, the state is inconsistent and startup fails closed.
   If both are absent, obtain 32 bytes from the OS CSPRNG, reject an invalid
   scalar, normalize it to even-Y, write only that normalized scalar to a
   private temporary file with restrictive permissions, fsync it, atomically
   rename it to `identity.key`, fsync the parent, and derive the x-only public
   key and node ID in memory. A lock held across this sequence makes concurrent
   first starts converge on one identity.
4. If identity, database, or metadata is corrupt, insecure, a symlink, owned by
   the wrong principal, or from an unsupported version, fail closed. Do not
   replace it, generate a second identity, repair trust, or continue with an
   empty database. A corrupt database is therefore never hidden by generating
   a replacement identity.
5. Create or migrate `node.sqlite` only after identity validation. A newly
   created database contains no peers, managers, enrollment, or trust rows.
   Identity creation never creates peer trust or enrollment.

Normal upgrades preserve the normalized `identity.key`, `node.sqlite`,
revocations, and the node ID. There is no persisted public-key companion file.
A migration is a numbered, transactional forward migration; a newer database,
failed migration, or downgrade attempt stops the service. Uninstall
removes the binary and service registration but preserves node state by default.
Reset is a separately confirmed operation that stops the service, archives or
deletes the entire node state as requested, and does not create a replacement
identity until a later start. Reinstall creates a fresh identity unless an
operator explicitly performs a validated recovery import.

Images must not contain `identity.key` or `node.sqlite`. A cloned state
directory is not a supported clone workflow: the service refuses conflicting
machine binding metadata when available, and an operator must reset the clone
before start. It must never silently regenerate a key or inherit trust. A
recovery import requires an offline-protected private key, an explicit
operator action, validation of its derived public key and node ID, and an
authorized trust decision; restoring a file alone is not enrollment.

Key rotation is explicit. The node generates or imports a scalar, normalizes it
to even-Y before persistence, records the old node ID as revoked with a reason
and timestamp, and activates the new identity only after the replacement trust
transition is durably recorded.
Offline recovery uses a previously configured recovery authority or a second
manager; relay availability cannot recover trust. Revocation records are
retained indefinitely unless the confirmed reset destroys the whole node
state.

## Trust state and optional network contract

`node.sqlite` is owned exclusively by the node service and is never placed in a
script workspace. It is a dedicated trust and delivery store, distinct from
`.history/runs.sqlite`. SQLite locking plus one service writer is required;
WAL and a bounded busy timeout may be used, but a lock timeout is an error, not
a reason to bypass the database. Corruption, an invalid schema version, failed
integrity check, or an unreadable WAL fails closed and preserves the files for
forensic recovery.

Schema version 1 has no unspecified security fields:

- `metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL)` stores exactly
  `schema_version = "1"`, the active node ID, and
  `public_key_encoding = "x-only-bip340-hex-lowercase"`. No other metadata
  keys are accepted.
- `peers(node_id TEXT PRIMARY KEY, public_key TEXT NOT NULL, role TEXT NOT NULL,
  state TEXT NOT NULL, capabilities_json TEXT NOT NULL, added_at TEXT NOT NULL,
  updated_at TEXT NOT NULL, last_seen TEXT NULL, source TEXT NOT NULL)` requires
  canonical x-only key/ID pairing, RFC3339 UTC timestamps, a sorted unique JSON string
  array of capability names, `role` of `conductor` or `performer`, `state` of
  `pending`, `active`, `suspended`, or `revoked`, and `source` of `manual`,
  `bundle`, or `recovery`. `active` is the only state allowed to authorize
  work.
- `revocations(id INTEGER PRIMARY KEY, node_id TEXT NOT NULL, public_key TEXT
  NOT NULL, revoked_at TEXT NOT NULL, reason TEXT NOT NULL,
  replacement_node_id TEXT NULL)` is append-only and retained for replay and
  old-key rejection. `replacement_node_id` is set only for an explicit
  rotation or recovery transition.
- `replay_keys(key TEXT PRIMARY KEY, first_seen TEXT NOT NULL, expires_at TEXT
  NOT NULL)` is retained until `expires_at`; rows may be deleted only after
  expiry, with protocol expiry capped at 24 hours and a fixed 48-hour clock
  skew retention window. Reconnect never clears unexpired rows.
- `inbox(cue_id TEXT PRIMARY KEY, state TEXT NOT NULL, received_at TEXT NOT
  NULL, updated_at TEXT NOT NULL, expires_at TEXT NOT NULL, outcome_hash TEXT
  NULL)` is the local durable delivery record. `state` is exactly `received`,
  `accepted`, `running`, `succeeded`, `failed`, `rejected`, `expired`, or
  `interrupted`; only `accepted` may enter `running`, and a recovered
  `running` row becomes `interrupted` rather than running again.

No peer becomes `active` without a signed, authenticated enrollment or an
explicit recovery operation authorized by an already trusted authority. A
relay announcement, matching node ID, config entry, successful transport
handshake, or first start cannot insert active trust. There is no implicit
"trust on first use" and no silent trust creation.

The future network modes are `direct`, `direct-with-nostr-fallback`, and
`nostr`. Direct transport is preferred where configured. Nostr is an opt-in,
untrusted delivery backend and never the authority, queue, or database. Relays
may retain, omit, delay, duplicate, reorder, or censor compact encrypted
events. They carry enrollment messages, discovery Beacons, Cues, acknowledgments,
compact outcomes, and Signals; scripts, plaintext secrets, backups, inventory
blobs, and large logs use direct or authenticated content channels instead.
Application envelopes are versioned as `omakure/1`; NIP-59 gift wrapping and
NIP-44 encryption are transport primitives, not authorization.

Enrollment is explicit: a short-lived high-entropy token and nonce are checked
for signature, sender, expiry, replay, and rate limits; the first completed
exchange wins; a second manager requires an authorized update. Pending offline
enrollment is not trust. A Cue has a unique ID, target and manager IDs,
operation/Battery hash, secret references, not-before/expiry, capability and
execution policy. Delivery is at least once, while durable local state makes
execution at most once; a crash in `running` becomes `interrupted`, never an
automatic retry of a possibly non-idempotent side effect.

Future MDM capabilities are typed, signed, versioned, auditable, and opt-in:
remote execution, baseline deployment, inventory/health, notifications, SSH
credential rotation, backup orchestration, and lost-device revocation. Campaign
targets are immutable snapshots with canaries, bounded batches, deadlines,
failure pauses, resource locks, and rollback. Destructive operations use typed
capabilities, exact targets, short-lived nonces, local audit, and configurable
independent approval quorum. Screen recording, keylogging, location tracking,
and productivity scoring are excluded.

## Threat model and security responses

| Threat | Frozen response |
|---|---|
| Private-key leakage | Raw key is service-owned `0600`/ACL-protected state; never logs, config, workspace, relay, or outcome; compromise requires explicit rotation/recovery. |
| Symlink/path attack | Absolute platform paths, no-follow checks, owner/mode/ACL validation, private atomic creation, and rejection of workspace-relative fallbacks. |
| Concurrent initialization | Exclusive state lock spans validation, CSPRNG generation, fsync, rename, and database creation; loser reopens and validates the winner. |
| Rollback or downgrade | Strict config/database versions, transactional forward-only migrations, retained revocations, and refusal of newer or downgraded state. |
| Workspace copying | Node state is outside the workspace; images exclude identity/trust; copied state requires explicit recovery or reset and never silently inherits trust. |
| Unauthorized trust mutation | Only authenticated enrollment/recovery paths may write active trust; relay data and transport handshakes cannot do so; append-only revocations and replay keys remain local. |
| Corruption or insecure permissions | Fail closed and preserve evidence; never replace corrupt identity/database with fresh state. |
| Malicious relay or replay | End-to-end authenticated envelopes, expiry, nonces, local replay keys, durable Cue IDs, and direct-channel preference. |

Future implementation tasks must add tests for these failure modes before any
remote-management or MDM feature is accepted. This document freezes the node
foundation and future direction; direct transport, discovery, enrollment, and
the minimal Health Plane are shipped only within the bounded foundation
described above. The direct transport gate and the Health Plane gate
(`mise run transport-certification` and `mise run health-plane-certification`)
are the authoritative stop records for this phase.
