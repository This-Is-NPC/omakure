# syntax=docker/dockerfile:1
#
# Official multi-stage image for `omakure engine`.
# Default runtime includes bash/git/jq (required script runtimes).
# Python and PowerShell variants are deferred — see .docs/deployment.md.

FROM rust:1-bookworm AS builder

WORKDIR /src

# Cache dependency builds when only sources change.
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY themes ./themes
# Integration tests are not needed in the image binary, but the package
# layout may reference them; keep the tree minimal via .dockerignore.

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
    && mkdir -p /workspace \
    && chown omakure:omakure /workspace

COPY --from=builder /src/target/release/omakure /usr/local/bin/omakure

ENV OMAKURE_SCRIPTS_DIR=/workspace
WORKDIR /workspace
USER omakure

EXPOSE 7878

# tini reaps zombies from script children and forwards signals for graceful
# engine shutdown (SIGTERM/SIGINT).
#
# CMD binds 0.0.0.0 inside the container so published ports work. Host-side
# publish should stay on 127.0.0.1 (see compose.yaml / .docs/deployment.md).
# Prefer OMAKURE_TOKENS_FILE over legacy OMAKURE_API_TOKEN in production.
ENTRYPOINT ["tini", "--", "omakure"]
CMD ["engine", "--bind", "0.0.0.0:7878", "--allow-non-loopback"]
