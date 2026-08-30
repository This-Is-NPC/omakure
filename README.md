<p align="center">
  <img src="docs/assets/brand/omakure-header.gif" width="1200" alt="Animated Omakure wordmark: connected pixel nodes pulse, form an X, merge into the letter O, and fan open into OMAKURE">
</p>

> ### *Scripts, nodes... and control.*

Omakure is a self-hosted remote operations layer for managing a fleet of
computers over a secure peer-to-peer mesh.

Distribute approved automation, run remote maintenance, monitor operational
status, and audit executions across Linux, macOS, and Windows.

## How it works

Computers connect directly over authenticated, encrypted sessions. No fixed
coordination server is required, and every relationship defines what each peer
is allowed to request or report.

The protocol gives each participant a clear responsibility:

| Role | Protocol term | Responsibility |
|---|---|---|
| **Control node** | Conductor | Monitors managed computers, requests approved operations, and delivers signed automation releases. |
| **Managed computer** | Performer | Reports current status, validates incoming requests, and runs local automation. |
| **Signing authority** | Publisher | Signs the exact automation files approved for distribution. |

A computer can control some peers while being managed by another. Its role is
defined by each trust relationship, not permanently assigned to the machine.

![A six-step animation showing a control node and managed computer sharing one authenticated session for fleet status, signed automation delivery, a remote operation, and its audit trail](docs/assets/three-planes-one-session.gif)

One authenticated session supports three independent workflows:

| Workflow | Protocol name | Purpose |
|---|---|---|
| **Fleet status** | Health | Reports liveness, runtime availability, recent outcomes, and the state of installed automation. |
| **Remote operation** | Cue | Requests an operation the managed computer already has and has explicitly approved. |
| **Signed automation delivery** | Baseline | Installs a signed, versioned set of automation files as one atomic release. |

## Remote operations run approved code

![A six-step animation showing a remote operation request crossing an authenticated session, passing local authorization checks, binding to an approved script, executing at most once, and reporting the result](docs/assets/cue-names-never-carries.gif)

A remote request identifies an operation by name. It does not contain source
code, arguments, environment values, secrets, or a working directory. The
managed computer checks its own permissions and allow-list, binds the request to
the exact local file, executes it at most once, and returns a bounded outcome.

New automation follows a separate path. A signed automation release is verified
and installed as a complete set, while a local operator can inspect and install
selected scripts from an automation repository. Software delivery and execution
remain separate operations.

## Built around scripts

Operations are ordinary Bash, PowerShell, Python, or embedded Lua scripts. Each
script can declare typed inputs, outputs, tags, secrets, and an optional cron
schedule through an embedded JSON schema.

The same execution path serves direct runs, local queues, schedules, the HTTP
API, and approved remote requests. Timeouts, cancellation, redaction, history,
and structured traces behave consistently across every entry point.

## Available today

| Area | Capabilities |
|---|---|
| **Fleet coordination** | Persistent machine identity, explicit trust, enrollment, revocation, LAN discovery, current status, and remote operations. |
| **Automation delivery** | Signed versioned releases, atomic installation, drift detection, and verified local rollback. |
| **Execution** | Direct runs, SQLite queues, concurrent workers, cron scheduling, cancellation, timeouts, redaction, history, and traces. |
| **Interfaces** | Human-readable CLI, stable JSON responses, authenticated HTTP API, and the machine-owned `node serve` process. |
| **Automation repositories** | Register, sync, inspect, validate, and locally install selected scripts with provenance on Unix systems. |
| **Platforms** | Build and release targets for Linux, macOS, and Windows, with native platform adapters and scripts for host-specific operations. |

This supports software updates, VPN and certificate maintenance, backups,
configuration changes, diagnostics, scheduled maintenance, and other operations
defined by the scripts your organization approves.

Specialized operations such as lock, wipe, recovery, or host checks are also
organization-owned scripts, not separate Omakure features. They can be maintained
in a private automation repository and distributed through signed releases.

The authenticated HTTP API also allows teams to build their own web, desktop,
CI, or agent-driven control surfaces without coupling them to the runtime.

## Security model

Omakure answers four operational questions:

- Which machine is this?
- Which peer may request an operation?
- Which exact script is approved to run?
- What happened when it ran?

Authorization belongs to the receiving computer. An encrypted connection proves
who is connected, but it does not grant permission by itself. Trust, capabilities,
approved scripts, signing authorities, and revocations are read from local state
before an operation is accepted.

Detailed output, traces, environment values, workspace paths, and secrets remain
on the computer that ran the operation. Fleet status and completion reports carry
only bounded operational data.

> Omakure audits operations, not people.

## Deployment patterns

Omakure does not require a single topology. The same computer can be managed by
one peer while controlling others. Every connection remains direct and keeps its
own trust and permissions.

### One central control node

A single control node can operate a small or medium fleet. A separate signing
authority approves the automation distributed through it.

```mermaid
flowchart TB
    signing["Signing authority"] -->|approved releases| control["Central control node"]
    control -->|remote operations| nodeA["Managed computer A"]
    control -->|remote operations| nodeB["Managed computer B"]
    control -->|remote operations| nodeC["Managed computer C"]
    nodeA -->|status and outcomes| control
    nodeB -->|status and outcomes| control
    nodeC -->|status and outcomes| control
```

### Multiple control nodes

Larger fleets can be split by team, site, network, or responsibility. Each
control node manages its own computers, while the same signing authority can
approve automation for every group.

```mermaid
flowchart TB
    signing["Signing authority"] -->|approved releases| controlA["Control node A"]
    signing -->|approved releases| controlB["Control node B"]

    subgraph siteA["Site or team A"]
        controlA -->|remote operations| nodeA1["Managed computer A1"]
        controlA -->|remote operations| nodeA2["Managed computer A2"]
        nodeA1 -->|status and outcomes| controlA
        nodeA2 -->|status and outcomes| controlA
    end

    subgraph siteB["Site or team B"]
        controlB -->|remote operations| nodeB1["Managed computer B1"]
        controlB -->|remote operations| nodeB2["Managed computer B2"]
        nodeB1 -->|status and outcomes| controlB
        nodeB2 -->|status and outcomes| controlB
    end
```

### Layered mesh

A computer can report to an upstream control node and manage its own downstream
group at the same time. This supports regional, site, or edge control without
changing the protocol.

```mermaid
flowchart TB
    root["Fleet control node"] -->|remote operations| regionA["Regional control node A"]
    root -->|remote operations| regionB["Regional control node B"]
    regionA -->|status and outcomes| root
    regionB -->|status and outcomes| root

    regionA -->|remote operations| edgeA1["Managed computer A1"]
    regionA -->|remote operations| edgeA2["Managed computer A2"]
    edgeA1 -->|status and outcomes| regionA
    edgeA2 -->|status and outcomes| regionA

    regionB -->|remote operations| edgeB1["Managed computer B1"]
    regionB -->|remote operations| edgeB2["Managed computer B2"]
    edgeB1 -->|status and outcomes| regionB
    edgeB2 -->|status and outcomes| regionB
```

Each level sees and operates its directly trusted peers. Operations, status, and
queues are not forwarded implicitly between levels.

## Roadmap

These are future directions, not current capabilities:

- **Omakure Control:** organizations, groups of computers, fleet-wide campaigns,
  access control, and a dedicated operations console.
- **Provisioned systems:** ISO images that boot as managed computers, generate
  their own identity, and enter the enrollment flow.
- **Windows automation repositories:** install approved scripts from automation
  repositories (Batteries) on Windows with the same validation, provenance, and
  safe replacement guarantees available on Unix.
- **Identity integration:** OIDC for the control console and optional integration
  with machine login flows.

## Quick start

Development requires Rust, Git, Bash, and `jq`. PowerShell and Python are needed
only for scripts using those runtimes; Lua 5.4 is embedded in Omakure.

Build the project and inspect the command surface:

```bash
cargo build
cargo test --locked
cargo run --bin omakure -- --help
cargo run --bin omakure -- help-ai
```

Create and run a local operation:

```bash
cargo run --bin omakure -- init hello.lua \
  --schema-json '{"Name":"hello","Fields":[]}' \
  --body-stdin <<'LUA'
print("hello from " .. _VERSION)
LUA

cargo run --bin omakure -- --json run hello.lua --actor local --reason smoke-test
cargo run --bin omakure -- --json history list --limit 5
```

Install the latest tagged release on Linux or macOS:

```bash
curl -fsSL https://raw.githubusercontent.com/This-Is-NPC/omakure/main/install.sh \
  | bash -s -- --repo This-Is-NPC/omakure
```

After installation, run `omakure doctor` to verify the workspace, required
tools, optional runtimes, and discovered script schemas.

## Documentation

- [Fleet model](docs/fleet-model.md)
- [CLI and HTTP usage](docs/usage.md)
- [Installation and machine services](docs/installation.md)
- [Deployment and security](docs/deployment.md)
- [Automation repositories](docs/batteries.md)
- [Script authoring](docs/how-to-create-a-script.md)
- [Architecture](docs/internal/architecture.md)
- [Complete documentation map](docs/README.md)

## Validation

```bash
cargo test --all-targets --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
```

## License

Omakure is available under the [MIT License](LICENSE).
