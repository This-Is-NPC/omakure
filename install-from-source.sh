#!/usr/bin/env bash
set -euo pipefail

APP_NAME="omakure"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PREFIX="${PREFIX:-$HOME/.local}"
BIN_DIR="${BIN_DIR:-${PREFIX}/bin}"
DOCUMENTS_DIR="${DOCUMENTS_DIR:-$HOME/Documents}"
SCRIPTS_DIR_ENV_SET=0
if [[ -n "${SCRIPTS_DIR+x}" ]]; then
  SCRIPTS_DIR_ENV_SET=1
fi
SCRIPTS_DIR_DEFAULT="${DOCUMENTS_DIR}/omakure-scripts"
SCRIPTS_DIR="${SCRIPTS_DIR:-${SCRIPTS_DIR_DEFAULT}}"
if [[ ${SCRIPTS_DIR_ENV_SET} -eq 0 ]]; then
  legacy_dirs=(
    "${DOCUMENTS_DIR}/overture-scripts"
    "${DOCUMENTS_DIR}/cloud-mgmt-scripts"
  )
  for legacy_dir in "${legacy_dirs[@]}"; do
    if [[ ! -d "${SCRIPTS_DIR_DEFAULT}" && -d "${legacy_dir}" ]]; then
      SCRIPTS_DIR="${legacy_dir}"
      break
    fi
  done
fi

sync_repo_scripts() {
  local source_dir="${SCRIPT_DIR}/scripts"
  local copied=0
  local skipped=0

  if [[ ! -d "${source_dir}" ]]; then
    return 0
  fi

  while IFS= read -r -d '' file; do
    local rel="${file#${source_dir}/}"
    local target="${SCRIPTS_DIR}/${rel}"
    if [[ -e "${target}" ]]; then
      skipped=$((skipped + 1))
      continue
    fi
    mkdir -p "$(dirname "${target}")"
    cp -p "${file}" "${target}"
    copied=$((copied + 1))
  done < <(find "${source_dir}" -type f -print0)

  if (( copied > 0 )); then
    echo "Copied ${copied} script(s) to ${SCRIPTS_DIR}"
  fi
  if (( copied == 0 && skipped > 0 )); then
    echo "Scripts already up to date in ${SCRIPTS_DIR}"
  fi
}

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo not found. Install Rust first: https://rustup.rs" >&2
  exit 1
fi

mkdir -p "${BIN_DIR}"
mkdir -p "${SCRIPTS_DIR}"

echo "Building ${APP_NAME}..."
cargo build --release --bin "${APP_NAME}" --manifest-path "${SCRIPT_DIR}/Cargo.toml"

echo "Installing to ${BIN_DIR}/${APP_NAME}..."
cp "${SCRIPT_DIR}/target/release/${APP_NAME}" "${BIN_DIR}/${APP_NAME}"
chmod +x "${BIN_DIR}/${APP_NAME}"

sync_repo_scripts

if ! echo ":${PATH}:" | grep -q ":${BIN_DIR}:"; then
  echo "Warning: ${BIN_DIR} is not in your PATH." >&2
  echo "Add this to your shell profile:" >&2
  echo "  export PATH=\"${BIN_DIR}:\\$PATH\"" >&2
fi

echo "Scripts folder: ${SCRIPTS_DIR}"
echo "Done. Run '${APP_NAME}' from your terminal."
