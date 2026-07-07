#!/usr/bin/env bash
# Dev helper: build, (re)start the daemon, tail its log, and open the TUI.
# On exit, stops the daemon so we don't leave orphans behind.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE="${OMAKURE_DEV_WORKSPACE:-$REPO_DIR/scripts}"

BIN="$REPO_DIR/target/debug/omakure"
LOG="$WORKSPACE/.omakure/daemon.log"

(cd "$REPO_DIR" && cargo build --bin omakure)

cd "$WORKSPACE"

"$BIN" serve --stop >/dev/null 2>&1 || true

mkdir -p .omakure
: > "$LOG"

"$BIN" serve -d
echo "daemon started — log: $LOG"

TAIL_PID=""
if [[ "${OMAKURE_DEV_TAIL:-1}" != "0" ]]; then
  tail -f "$LOG" &
  TAIL_PID=$!
fi

cleanup() {
  if [[ -n "$TAIL_PID" ]]; then
    kill "$TAIL_PID" 2>/dev/null || true
  fi
  "$BIN" serve --stop >/dev/null 2>&1 || true
  echo "daemon stopped"
}
trap cleanup EXIT INT TERM

"$BIN"
