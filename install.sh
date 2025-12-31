#!/usr/bin/env bash
set -euo pipefail

APP_NAME="omakure"
REPO_DEFAULT="This-Is-NPC/omakure"
REPO="${REPO:-$REPO_DEFAULT}"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
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
VERSION="${VERSION:-}"

usage() {
  cat <<USAGE
Usage: install.sh [--repo owner/name] [--version vX.Y.Z] [--bin-dir path]

Environment variables:
  REPO     GitHub repository, e.g. org/omakure
  VERSION  Release tag, e.g. v0.1.0 (defaults to latest)
  BIN_DIR  Install directory (default: ~/.local/bin)
  DOCUMENTS_DIR  Documents directory (default: ~/Documents)
  SCRIPTS_DIR  Scripts directory (default: ~/Documents/omakure-scripts)
USAGE
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo)
      REPO="$2"
      shift 2
      ;;
    --version)
      VERSION="$2"
      shift 2
      ;;
    --bin-dir)
      BIN_DIR="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown arg: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ -z "${REPO}" ]]; then
  echo "Missing REPO value." >&2
  exit 1
fi

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

download() {
  local url="$1"
  local dest="$2"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$dest"
  elif command -v wget >/dev/null 2>&1; then
    wget -q "$url" -O "$dest"
  else
    echo "Missing curl or wget" >&2
    exit 1
  fi
}

download_stdout() {
  local url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- "$url"
  else
    echo "Missing curl or wget" >&2
    exit 1
  fi
}

fetch_latest_version() {
  local repo="$1"
  local json
  json="$(download_stdout "https://api.github.com/repos/${repo}/releases/latest")"
  printf '%s' "$json" | tr -d '\r' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1
}

sync_repo_scripts() {
  local repo="$1"
  local version="$2"
  local dest_dir="$3"
  local work_dir="$4"
  local source_url="https://github.com/${repo}/archive/refs/tags/${version}.tar.gz"
  local source_tar="${work_dir}/source.tar.gz"
  local source_root="${work_dir}/source"
  local scripts_src
  local copied=0
  local skipped=0

  set +e
  download "${source_url}" "${source_tar}"
  local download_status=$?
  set -e

  if [[ ${download_status} -ne 0 ]]; then
    echo "Warning: failed to download scripts from ${source_url}" >&2
    return 0
  fi

  mkdir -p "${source_root}"
  if ! tar -xzf "${source_tar}" -C "${source_root}"; then
    echo "Warning: failed to unpack scripts archive" >&2
    return 0
  fi

  scripts_src="$(find "${source_root}" -maxdepth 2 -type d -name scripts | head -n1)"
  if [[ -z "${scripts_src}" || ! -d "${scripts_src}" ]]; then
    echo "Warning: scripts folder not found in source archive" >&2
    return 0
  fi

  while IFS= read -r -d '' file; do
    local rel="${file#${scripts_src}/}"
    local target="${dest_dir}/${rel}"
    if [[ -e "${target}" ]]; then
      skipped=$((skipped + 1))
      continue
    fi
    mkdir -p "$(dirname "${target}")"
    cp -p "${file}" "${target}"
    copied=$((copied + 1))
  done < <(find "${scripts_src}" -type f -print0)

  if (( copied > 0 )); then
    echo "Copied ${copied} script(s) to ${dest_dir}"
  fi
  if (( copied == 0 && skipped > 0 )); then
    echo "Scripts already up to date in ${dest_dir}"
  fi
}

require_cmd tar

if [[ -z "${VERSION}" ]]; then
  VERSION="$(fetch_latest_version "${REPO}")"
fi

if [[ -z "${VERSION}" ]]; then
  echo "Failed to resolve release version" >&2
  exit 1
fi

case "$(uname -s)" in
  Linux)
    os="linux"
    ;;
  Darwin)
    os="darwin"
    ;;
  *)
    echo "Unsupported OS: $(uname -s)" >&2
    exit 1
    ;;
 esac

case "$(uname -m)" in
  x86_64|amd64)
    arch="x86_64"
    ;;
  arm64|aarch64)
    arch="aarch64"
    ;;
  *)
    echo "Unsupported architecture: $(uname -m)" >&2
    exit 1
    ;;
 esac

asset="${APP_NAME}-${VERSION}-${os}-${arch}.tar.gz"
url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"

tmp_dir="$(mktemp -d)"
trap 'rm -rf "${tmp_dir}"' EXIT

download "${url}" "${tmp_dir}/${asset}"

tar -xzf "${tmp_dir}/${asset}" -C "${tmp_dir}"

bin_path="${tmp_dir}/${APP_NAME}"
if [[ ! -f "${bin_path}" ]]; then
  bin_path="$(find "${tmp_dir}" -maxdepth 2 -type f -name "${APP_NAME}" | head -n1)"
fi

if [[ -z "${bin_path}" || ! -f "${bin_path}" ]]; then
  echo "Binary not found in archive" >&2
  exit 1
fi

mkdir -p "${BIN_DIR}"
mkdir -p "${SCRIPTS_DIR}"
cp "${bin_path}" "${BIN_DIR}/${APP_NAME}"
chmod +x "${BIN_DIR}/${APP_NAME}"

sync_repo_scripts "${REPO}" "${VERSION}" "${SCRIPTS_DIR}" "${tmp_dir}"

if ! echo ":${PATH}:" | grep -q ":${BIN_DIR}:"; then
  echo "Warning: ${BIN_DIR} is not in your PATH." >&2
  echo "Add this to your shell profile:" >&2
  echo "  export PATH=\"${BIN_DIR}:\\$PATH\"" >&2
fi

echo "Installed ${APP_NAME} ${VERSION} to ${BIN_DIR}/${APP_NAME}"
echo "Scripts folder: ${SCRIPTS_DIR}"
echo "Run '${APP_NAME}' from your terminal."
