#!/usr/bin/env bash
# Build and smoke-test the node service without leaving a daemon running.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE="${OMAKURE_DEV_WORKSPACE:-$REPO_DIR/scripts}"
BIN="$REPO_DIR/target/debug/omakure"
PORT="${OMAKURE_DEV_PORT:-17878}"
LOG="${WORKSPACE}/.omakure/dev-node.log"

(cd "$REPO_DIR" && cargo build --bin omakure)
mkdir -p "$WORKSPACE/.omakure/dev-node-state"

TOKEN="$(openssl rand -hex 32)"
export OMAKURE_API_TOKEN="$TOKEN"
READY_URL="http://127.0.0.1:${PORT}/v1/ready"
HEALTH_URL="http://127.0.0.1:${PORT}/v1/health"

OMAKURE_NODE_TEST_MODE=1 "$BIN" node --node-state-dir "$WORKSPACE/.omakure/dev-node-state" \
  --node-config "$WORKSPACE/.omakure/dev-node.toml" serve \
  --bind "127.0.0.1:${PORT}" --workers 0 --no-scheduler \
  --capability all >"$LOG" 2>&1 &
NODE_PID=$!

cleanup() {
  kill "$NODE_PID" 2>/dev/null || true
  wait "$NODE_PID" 2>/dev/null || true
}
trap cleanup EXIT INT TERM

for _ in {1..50}; do
  if curl -fsS "$HEALTH_URL" >/dev/null 2>&1; then
    break
  fi
  sleep 0.1
done

curl -fsS "$HEALTH_URL" >/dev/null
curl -fsS "$READY_URL" >/dev/null
curl -fsS -H "Authorization: Bearer ${TOKEN}" \
  "http://127.0.0.1:${PORT}/v1/scripts" >/dev/null

printf 'node service ready on %s (log: %s)\n' "127.0.0.1:${PORT}" "$LOG"
