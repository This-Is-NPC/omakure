# syntax=docker/dockerfile:1@sha256:ecfaec9ed6d810b56388c508f4121597bfbba70d41a6dfeee4d8cad5f295fc32
#
# Official multi-stage image for `omakure node serve`.
# Default runtime includes bash/git/jq (required script runtimes).
# Python and PowerShell variants are deferred — see docs/deployment.md.

FROM rust@sha256:e536cf316987faedfe8ae120f83b70c7df0068fdb4fc9efcce55c71a625001d5 AS builder

WORKDIR /src

# Cache dependency builds when only sources change.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY fixtures/cli-http-parity.toml fixtures/operation-catalog.toml ./fixtures/

RUN cargo build --release --bin omakure \
    && strip target/release/omakure

# The Health Plane attempt-exhaustion harness. It is built here, rather than on
# the host, so that `scripts/tasks/cert/health` can run it as a container on the
# certification network instead of as a host process. A host process would need
# the Performer's container to dial the host, which any default-deny INPUT
# firewall drops; inside the network the phase depends on
# nothing but Docker itself.
#
# These stages are deliberately placed before `runtime` so that the default
# build target stays `runtime` for every other caller. BuildKit skips them
# unless they are requested with `--target harness`.
FROM builder AS harness-builder

COPY tests ./tests

# Built in the debug profile deliberately, exactly like the host-side adversary
# harness. `NodeContext::resolve_for` refuses a state-directory override outside
# `cfg!(debug_assertions)` (the `test_mode && !cfg!(debug_assertions)` guard in
# src/node.rs), which is a security property of the shipped binary; a release
# harness cannot read the adversary's node material.
# Only the harness is affected -- the node under test is the release `runtime`
# image, unchanged.
#
# `cargo test --no-run` names the binary with a content hash, so resolve it
# rather than guessing. Exactly one non-`.d` file matches.
RUN cargo test --locked --test docker_health_plane_exhaustion --no-run \
    && harness=$(find target/debug/deps -maxdepth 1 -type f \
        -name 'docker_health_plane_exhaustion-*' ! -name '*.d' -print | head -n 1) \
    && [ -n "$harness" ] \
    && install -m 0755 "$harness" /usr/local/bin/health-plane-exhaustion-harness

FROM debian@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS harness

COPY --from=harness-builder /usr/local/bin/health-plane-exhaustion-harness /usr/local/bin/health-plane-exhaustion-harness

# The harness writes its readiness marker and reads the adversary's node
# material through a bind mount the runner owns; it needs no persistent state.
ENTRYPOINT ["/usr/local/bin/health-plane-exhaustion-harness"]
CMD ["--ignored", "--nocapture", "--exact", \
     "an_unacknowledged_profile_stops_at_the_frozen_attempt_budget_on_one_session"]

FROM debian@sha256:88200866dfff7ea7f5cbcb6ec7c8a701889efe6fe859fe64d6990e4b07ea4171 AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash=5.2.15-2+b13 \
        ca-certificates=20250419~deb12u1 \
        curl=7.88.1-10+deb12u15 \
        git=1:2.39.5-0+deb12u3 \
        jq=1.6-2.1+deb12u2 \
        tini=0.19.0-1+b3 \
    && rm -rf /var/lib/apt/lists/* \
    && groupadd --system --gid 10001 omakure \
    && useradd --system --uid 10001 --gid omakure --home-dir /workspace --shell /usr/sbin/nologin omakure \
     && mkdir -p /workspace /var/lib/omakure /etc/omakure \
     && printf '%s\n' \
          'version = 1' \
          '' \
          '[node]' \
          'display_name = ""' \
          '' \
          '[api]' \
          'bind = "127.0.0.1:7878"' \
          '' \
          '[network]' \
          'mode = "direct"' \
          'relays = []' \
          'static_peers = []' \
          'max_message_bytes = 1048576' \
          '' \
          '[trust]' \
          'enrollment = "disabled"' \
          'allow_remote_cues = false' \
          'remote_cue_scripts = []' \
          'remote_cue_batteries = []' \
          'allow_baseline_push = false' \
          'baseline_publishers = []' \
          'authorities = []' \
          'bootstrap_token_hash = ""' \
          'bootstrap_nonce_hash = ""' \
          '' \
          '[discovery]' \
          'enabled = false' \
          'port = 38383' \
          'multicast_addr = "239.255.42.99"' \
          'broadcast = true' \
          '' \
          '[organization]' \
          'id = ""' \
          'discovery_secret_ref = ""' > /etc/omakure/node.toml \
     && chown root:omakure /etc/omakure/node.toml \
     && chmod 0640 /etc/omakure/node.toml \
     && chown -R omakure:omakure /workspace /var/lib/omakure \
     && chmod 0750 /workspace \
     && chmod 0700 /var/lib/omakure

COPY --from=builder /src/target/release/omakure /usr/local/bin/omakure

ENV OMAKURE_SCRIPTS_DIR=/workspace
WORKDIR /workspace
USER omakure

EXPOSE 7878

# tini reaps zombies from script children and forwards signals for graceful
# node-service shutdown (SIGTERM/SIGINT).
#
# CMD binds 0.0.0.0 inside the container so published ports work. Host-side
# publish should stay on 127.0.0.1 (see compose.yaml / docs/deployment.md).
# Prefer OMAKURE_TOKENS_FILE over legacy OMAKURE_API_TOKEN in production.
ENTRYPOINT ["tini", "--", "omakure"]
CMD ["node", "serve", "--bind", "0.0.0.0:7878", "--allow-non-loopback"]
