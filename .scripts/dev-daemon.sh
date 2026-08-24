#!/usr/bin/env bash
# Build and smoke-test the headless engine without leaving a daemon running.

set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE="${OMAKURE_DEV_WORKSPACE:-$REPO_DIR/scripts}"
BIN="$REPO_DIR/target/debug/omakure"
PORT="${OMAKURE_DEV_PORT:-17878}"
LOG="${WORKSPACE}/.omakure/dev-engine.log"

(cd "$REPO_DIR" && cargo build --bin omakure)
mkdir -p "$WORKSPACE/.omakure"

TOKEN="$(openssl rand -hex 32)"
export OMAKURE_API_TOKEN="$TOKEN"
READY_URL="http://127.0.0.1:${PORT}/v1/ready"
HEALTH_URL="http://127.0.0.1:${PORT}/v1/health"

"$BIN" engine --bind "127.0.0.1:${PORT}" --workers 0 --no-scheduler \
  --capability all >"$LOG" 2>&1 &
ENGINE_PID=$!

cleanup() {
  kill "$ENGINE_PID" 2>/dev/null || true
  wait "$ENGINE_PID" 2>/dev/null || true
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

printf 'headless engine ready on %s (log: %s)\n' "127.0.0.1:${PORT}" "$LOG"
