# Direct Transport: Docker

**Status:** Manual; invoked by the Linux transport certification, not by default `cargo test`.

## Source

- `scripts/tasks/cert/direct-transport`
- `ci/compose/compose.direct-transport.e2e.yaml`
- Docker-side support in `tests/direct_transport_e2e.rs`

## Run

```bash
docker build --tag omakure-direct-transport-e2e:local .
scripts/tasks/cert/direct-transport
```

The script itself uses a project-specific Compose name and 120-second Docker
command bounds. It creates direct-a, direct-b, and direct-c; only the first two
run services, while direct-c is the adversary/probe client.

## Proves

- Three packaged containers initialize independent identities and token files.
- Encrypted direct probes work for trusted inbound peers, reject mismatched targets and mismatched static locators, and record accepted/rejected audits.
- A stopped listener causes the continuously running dialer to observe disconnection and reconnect on a new session.
- Revocation prevents a fresh session.

## Does Not Prove

- It does not certify discovery, enrollment, Cue, or baseline semantics.
- It is not a persistent Compose deployment sample.

## Cleanup and Troubleshooting

The EXIT cleanup runs `docker compose down --volumes --remove-orphans`, inspects
project-labeled containers/networks/volumes, and removes its temporary directory.
If a resource remains, inspect it before manually removing anything.
