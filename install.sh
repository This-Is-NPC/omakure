#!/usr/bin/env bash
set -euo pipefail

APP_NAME="omakure"
REPO_DEFAULT="This-Is-NPC/omakure"
REPO="${REPO:-$REPO_DEFAULT}"
BIN_DIR_ENV_SET=0
if [[ -n "${BIN_DIR+x}" ]]; then
  BIN_DIR_ENV_SET=1
fi
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
INSTALL_NODE_SERVICE=0
UNINSTALL_NODE_SERVICE=0
RESET_NODE_STATE=0
CONFIRMED=0
NODE_TOKENS_FILE="${NODE_TOKENS_FILE:-}"

usage() {
  cat <<USAGE
Usage: install.sh [--repo owner/name] [--version vX.Y.Z] [--bin-dir path]
                  [--install-node-service --node-tokens-file path]
                  [--uninstall-node-service [--uninstall-node-state --confirmed]]

Environment variables:
  REPO     GitHub repository, e.g. org/omakure
  VERSION  Release tag, e.g. v0.1.0 (defaults to latest)
  BIN_DIR  Install directory (default: ~/.local/bin)
  DOCUMENTS_DIR  Documents directory (default: ~/Documents)
  SCRIPTS_DIR  Scripts directory (default: ~/Documents/omakure-scripts)
  --install-node-service  Opt into privileged machine-service provisioning.
  --node-tokens-file path  Existing hashed tokens TOML for the machine service.
  --uninstall-node-service  Remove only the native service registration.
  --uninstall-node-state --confirmed  Also remove machine node state/config.
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
      BIN_DIR_ENV_SET=1
      shift 2
      ;;
    --install-node-service)
      INSTALL_NODE_SERVICE=1
      shift
      ;;
    --uninstall-node-service)
      UNINSTALL_NODE_SERVICE=1
      shift
      ;;
    --uninstall-node-state)
      RESET_NODE_STATE=1
      shift
      ;;
    --node-tokens-file)
      NODE_TOKENS_FILE="$2"
      shift 2
      ;;
    --confirmed)
      CONFIRMED=1
      shift
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

if (( INSTALL_NODE_SERVICE && UNINSTALL_NODE_SERVICE )); then
  echo "--install-node-service and --uninstall-node-service cannot be combined." >&2
  exit 1
fi
if (( RESET_NODE_STATE && !UNINSTALL_NODE_SERVICE )); then
  echo "--uninstall-node-state requires --uninstall-node-service." >&2
  exit 1
fi
if (( RESET_NODE_STATE && !CONFIRMED )); then
  echo "--uninstall-node-state requires --confirmed." >&2
  exit 1
fi
if (( INSTALL_NODE_SERVICE )) && [[ -z "${NODE_TOKENS_FILE}" || ! -f "${NODE_TOKENS_FILE}" ]]; then
  echo "--install-node-service requires an existing --node-tokens-file with hashed entries." >&2
  exit 1
fi
if (( INSTALL_NODE_SERVICE && BIN_DIR_ENV_SET == 0 )); then
  BIN_DIR="/usr/local/bin"
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

require_root() {
  if [[ "$(id -u)" -ne 0 ]]; then
    echo "Machine node-service provisioning requires root; normal installs remain per-user." >&2
    exit 1
  fi
}

validate_native_service_binary_path() {
  local binary="$1"
  if [[ "${binary}" != /* ]]; then
    echo "Native service binary path must be absolute: ${binary}" >&2
    exit 1
  fi
  case "${binary}" in
    *[!A-Za-z0-9._/:-]*)
      echo "Native service binary path contains unsupported system-service characters: ${binary}" >&2
      echo "Use a path containing only letters, digits, '.', '_', '/', ':', or '-'." >&2
      exit 1
      ;;
  esac
}

node_config_text() {
  cat <<'CONFIG'
version = 1

[node]
display_name = ""

[api]
bind = "127.0.0.1:7878"

[network]
mode = "direct"
relays = []
static_peers = []
max_message_bytes = 1048576

[trust]
enrollment = "disabled"
allow_remote_cues = false
allow_baseline_push = false

[discovery]
enabled = false
port = 38383
multicast_addr = "239.255.42.99"
broadcast = true

[organization]
id = ""
discovery_secret_ref = ""
CONFIG
}

write_node_config_if_missing() {
  local config_path="$1"
  local config_group="$2"
  local parent
  parent="$(dirname "${config_path}")"
  mkdir -p "${parent}"
  if [[ ! -e "${config_path}" ]]; then
    local temp
    temp="$(mktemp "${parent}/.node.toml.tmp.XXXXXX")"
    node_config_text >"${temp}"
    install -o root -g "${config_group}" -m 0640 "${temp}" "${config_path}"
    rm -f "${temp}"
  fi
  chown root:"${config_group}" "${config_path}"
  chmod 0640 "${config_path}"
}

install_node_tokens_if_requested() {
  local tokens_path="$1"
  local tokens_group="$2"
  if [[ -z "${NODE_TOKENS_FILE}" || ! -f "${NODE_TOKENS_FILE}" ]]; then
    echo "--node-tokens-file must name an existing hashed tokens TOML." >&2
    exit 1
  fi
  install -o root -g "${tokens_group}" -m 0640 "${NODE_TOKENS_FILE}" "${tokens_path}"
}

ensure_linux_node_principal() {
  if ! getent group omakure >/dev/null; then
    groupadd --system omakure
  fi
  if ! id omakure >/dev/null 2>&1; then
    useradd --system --gid omakure --home-dir /var/lib/omakure \
      --shell /usr/sbin/nologin omakure
  fi
}

install_linux_node_service() {
  local binary="$1"
  require_root
  validate_native_service_binary_path "${binary}"
  ensure_linux_node_principal
  mkdir -p /var/lib/omakure /var/lib/omakure-workspace /etc/omakure
  chown omakure:omakure /var/lib/omakure /var/lib/omakure-workspace
  chmod 0700 /var/lib/omakure
  chmod 0750 /var/lib/omakure-workspace
  write_node_config_if_missing /etc/omakure/node.toml omakure
  install_node_tokens_if_requested /etc/omakure/tokens.toml omakure
  cat >/etc/systemd/system/omakure-node.service <<UNIT
[Unit]
Description=Omakure machine node service
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=omakure
Group=omakure
Environment=OMAKURE_SCRIPTS_DIR=/var/lib/omakure-workspace
Environment=OMAKURE_TOKENS_FILE=/etc/omakure/tokens.toml
ExecStart=${binary} node serve
Restart=on-failure
NoNewPrivileges=true
PrivateTmp=true

[Install]
WantedBy=multi-user.target
UNIT
  chmod 0644 /etc/systemd/system/omakure-node.service
  systemctl daemon-reload
  systemctl enable omakure-node.service
  echo "Provisioned Linux service omakure-node.service; start it with systemctl start omakure-node."
}

ensure_macos_node_principal() {
  if ! id _omakure >/dev/null 2>&1; then
    dscl . -create /Groups/_omakure
    dscl . -create /Groups/_omakure PrimaryGroupID 450
    dscl . -create /Users/_omakure
    dscl . -create /Users/_omakure UserShell /usr/bin/false
    dscl . -create /Users/_omakure RealName "Omakure Node"
    dscl . -create /Users/_omakure UniqueID 450
    dscl . -create /Users/_omakure PrimaryGroupID 450
    dscl . -create /Users/_omakure NFSHomeDirectory "/Library/Application Support/Omakure"
  fi
}

install_macos_node_service() {
  local binary="$1"
  require_root
  validate_native_service_binary_path "${binary}"
  ensure_macos_node_principal
  local root="/Library/Application Support/Omakure"
  local workspace="/Library/Application Support/Omakure-Workspace"
  local config="${root}/node.toml"
  local tokens="${root}/tokens.toml"
  mkdir -p "${root}" "${workspace}"
  chown _omakure:_omakure "${root}" "${workspace}"
  chmod 0700 "${root}"
  chmod 0750 "${workspace}"
  write_node_config_if_missing "${config}" _omakure
  install_node_tokens_if_requested "${tokens}" _omakure
  cat >/Library/LaunchDaemons/com.omakure.node.plist <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>com.omakure.node</string>
  <key>UserName</key><string>_omakure</string>
  <key>ProgramArguments</key>
  <array><string>${binary}</string><string>node</string><string>serve</string></array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>OMAKURE_SCRIPTS_DIR</key><string>${workspace}</string>
    <key>OMAKURE_TOKENS_FILE</key><string>${tokens}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
PLIST
  chmod 0644 /Library/LaunchDaemons/com.omakure.node.plist
  launchctl bootout system/com.omakure.node >/dev/null 2>&1 || true
  launchctl bootstrap system /Library/LaunchDaemons/com.omakure.node.plist
  echo "Provisioned macOS LaunchDaemon com.omakure.node; it is loaded but node state was preserved."
}

uninstall_node_service() {
  require_root
  case "${os}" in
    linux)
      systemctl disable --now omakure-node.service >/dev/null 2>&1 || true
      rm -f /etc/systemd/system/omakure-node.service
      systemctl daemon-reload
      if (( RESET_NODE_STATE )); then
        rm -rf /var/lib/omakure /etc/omakure/node.toml /etc/omakure/tokens.toml
      fi
      ;;
    darwin)
      launchctl bootout system/com.omakure.node >/dev/null 2>&1 || true
      rm -f /Library/LaunchDaemons/com.omakure.node.plist
      if (( RESET_NODE_STATE )); then
        rm -rf "/Library/Application Support/Omakure" \
          "/Library/Application Support/Omakure-Workspace"
      fi
      ;;
  esac
  if (( RESET_NODE_STATE )); then
    echo "Removed the Omakure machine service registration and confirmed node state."
  else
    echo "Removed the Omakure machine service registration; node state was preserved."
  fi
}

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

if (( UNINSTALL_NODE_SERVICE )); then
  uninstall_node_service
  exit 0
fi

if (( INSTALL_NODE_SERVICE )); then
  validate_native_service_binary_path "${BIN_DIR}/${APP_NAME}"
fi

if [[ -z "${REPO}" ]]; then
  echo "Missing REPO value." >&2
  exit 1
fi

require_cmd tar

if [[ -z "${VERSION}" ]]; then
  VERSION="$(fetch_latest_version "${REPO}")"
fi

if [[ -z "${VERSION}" ]]; then
  echo "Failed to resolve release version" >&2
  exit 1
fi

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

if (( INSTALL_NODE_SERVICE )); then
  case "${os}" in
    linux) install_linux_node_service "${BIN_DIR}/${APP_NAME}" ;;
    darwin) install_macos_node_service "${BIN_DIR}/${APP_NAME}" ;;
  esac
fi

if ! echo ":${PATH}:" | grep -q ":${BIN_DIR}:"; then
  echo "Warning: ${BIN_DIR} is not in your PATH." >&2
  echo "Add this to your shell profile:" >&2
  echo "  export PATH=\"${BIN_DIR}:\\$PATH\"" >&2
fi

echo "Installed ${APP_NAME} ${VERSION} to ${BIN_DIR}/${APP_NAME}"
echo "Scripts folder: ${SCRIPTS_DIR}"
echo "Run '${APP_NAME}' from your terminal."
