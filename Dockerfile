# syntax=docker/dockerfile:1
#
# Official multi-stage image for `omakure node serve`.
# Default runtime includes bash/git/jq (required script runtimes).
# Python and PowerShell variants are deferred — see .docs/deployment.md.

FROM rust:1-bookworm AS builder

WORKDIR /src

# Cache dependency builds when only sources change.
COPY Cargo.toml Cargo.lock ./
COPY src ./src

RUN cargo build --release --bin omakure \
    && strip target/release/omakure

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        bash \
        ca-certificates \
        git \
        jq \
        tini \
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
          'allow_baseline_push = false' \
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
# publish should stay on 127.0.0.1 (see compose.yaml / .docs/deployment.md).
# Prefer OMAKURE_TOKENS_FILE over legacy OMAKURE_API_TOKEN in production.
ENTRYPOINT ["tini", "--", "omakure"]
CMD ["node", "serve", "--bind", "0.0.0.0:7878", "--allow-non-loopback"]
