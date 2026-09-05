#!/usr/bin/env bash
set -euo pipefail

APP_NAME="omakure"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-${PREFIX}/bin}"


if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust first: https://rustup.rs" >&2
  exit 1
fi

mkdir -p "${BIN_DIR}"

echo "Building ${APP_NAME}..."
cargo build --release --bin "${APP_NAME}" --manifest-path "${SCRIPT_DIR}/Cargo.toml"

echo "Installing to ${BIN_DIR}/${APP_NAME}..."
cp "${SCRIPT_DIR}/target/release/${APP_NAME}" "${BIN_DIR}/${APP_NAME}"
chmod +x "${BIN_DIR}/${APP_NAME}"


if ! echo ":${PATH}:" | grep -q ":${BIN_DIR}:"; then
  echo "Warning: ${BIN_DIR} is not in your PATH." >&2
  echo "Add this to your shell profile:" >&2
  echo "  export PATH=\"${BIN_DIR}:\\$PATH\"" >&2
fi

echo "Done. Run '${APP_NAME}' from your terminal."
