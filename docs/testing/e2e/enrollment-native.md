# Enrollment: Native

**Status:** CI for authority issuance/application; Docker transaction coverage is separate.

## Source

- `tests/enrollment_authority_e2e.rs`
- Enrollment implementation: `src/enrollment.rs`

## Run

```bash
cargo test --test enrollment_authority_e2e --locked
```

## Proves

- A real authority creates a public signing record and issues a bundle for an audience node.
- The audience accepts a valid product-issued bundle and records the issuer as an active peer with the requested role/capability.
- An unnamed authority and a revoked authority are refused.
- Re-running authority creation refuses rather than rotating the fleet key.
- Authority private material is absent from command output.

## Does Not Prove

- It does not prove a running unattended container can apply a bundle; see [Signed bundle Docker](signed-bundle-docker.md).
- It does not prove manual enrollment request/approval lifecycle; see [Enrollment Docker](enrollment-docker.md).

## Environment and Cleanup

Each test uses temporary issuer, stranger, and audience workspaces. Bootstrap
tokens and bundles are staged with restrictive permissions on Unix and removed
with their temporary directories.

## Troubleshooting

- Apply refused: verify the audience names the authority public key, organization, bootstrap token hash, and nonce hash.
- Private material in output: stop and treat it as a security regression; do not publish the output.
