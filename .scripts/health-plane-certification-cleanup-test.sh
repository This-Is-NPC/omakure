#!/usr/bin/env bash
set -euo pipefail

# Cleanup is verified for three shapes, all bounded:
#
#   1. partial startup - Compose creates the first node's resources and then a
#      sidecar exits non-zero under --abort-on-container-exit;
#   2. failure - the certification script exits non-zero from that point;
#   3. interrupt - the same run is sent SIGINT part-way through and must still
#      remove everything through its EXIT/INT/TERM trap.
#
# The success shape is verified by the certification command itself, which
# fails if any project resource survives its own teardown.

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

assert_removed() {
    local project=$1 shape=$2 resource remaining
    for resource in container network volume; do
        if ! remaining=$(timeout --foreground --kill-after=5s 30s docker "$resource" ls -q \
            --filter "label=com.docker.compose.project=$project"); then
            printf 'health-plane certification cleanup test: unable to inspect %s for %s\n' \
                "$resource" "$project" >&2
            exit 1
        fi
        if [[ -n "$remaining" ]]; then
            printf 'health-plane certification cleanup test: %s survived %s for %s\n' \
                "$resource" "$shape" "$project" >&2
            exit 1
        fi
    done
    printf 'health-plane certification cleanup test: %s left no resources behind\n' "$shape"
}

# 1 + 2: induced partial startup and failure.
induced_project="omakure-health-plane-certification-induced-${BASHPID}"
if OMAKURE_HEALTH_CERTIFICATION_PROJECT="$induced_project" \
    OMAKURE_HEALTH_CERTIFICATION_INDUCE_FAILURE=1 \
    timeout --foreground --kill-after=10s 30m "$root_dir/.scripts/health-plane-certification.sh"; then
    printf 'health-plane certification cleanup test: induced failure unexpectedly passed\n' >&2
    exit 1
fi
assert_removed "$induced_project" "an induced partial startup and failure"

# 3: interrupt part-way through a real run.
interrupt_project="omakure-health-plane-certification-interrupt-${BASHPID}"
OMAKURE_HEALTH_CERTIFICATION_PROJECT="$interrupt_project" \
    timeout --foreground --kill-after=10s 30m "$root_dir/.scripts/health-plane-certification.sh" \
    >/dev/null 2>&1 &
run_pid=$!
# Bounded: interrupt once the fleet is genuinely up, and never wait forever for
# it. The deadline is the gate's own readiness budget plus the image build.
deadline=$((SECONDS + 420))
started=0
while (( SECONDS < deadline )); do
    if ! kill -0 "$run_pid" 2>/dev/null; then
        break
    fi
    if [[ -n "$(timeout --foreground --kill-after=5s 30s docker container ls -q \
        --filter "label=com.docker.compose.project=$interrupt_project")" ]]; then
        started=1
        break
    fi
    sleep 5
done
if (( started == 0 )); then
    kill -INT "$run_pid" 2>/dev/null || true
    wait "$run_pid" 2>/dev/null || true
    printf 'health-plane certification cleanup test: the interrupt run never started containers\n' >&2
    exit 1
fi
kill -INT "$run_pid" 2>/dev/null || true
# Bash runs a trap only between commands, so a SIGINT that lands while the gate
# is inside a bounded Docker command is not observed until that command
# returns. The ceiling is therefore the longest single in-flight command (the
# 180s Compose bound) plus the whole teardown path: `compose down` (180s), the
# reclaim `docker run` (120s), and the three resource sweeps (120s each).
#
# Measured, not guessed: a SIGINT delivered during the Phase 1 `compose run`
# exits after ~360s and removes every container, network, and volume. The old
# 180s ceiling was below the cost of a single deferred command and failed a
# correct teardown. This is a ceiling, not a wait - the loop breaks as soon as
# the process is gone.
interrupt_deadline=$((SECONDS + 900))
while (( SECONDS < interrupt_deadline )); do
    kill -0 "$run_pid" 2>/dev/null || break
    sleep 2
done
if kill -0 "$run_pid" 2>/dev/null; then
    kill -TERM "$run_pid" 2>/dev/null || true
    wait "$run_pid" 2>/dev/null || true
    printf 'health-plane certification cleanup test: the interrupted run did not exit within its bound\n' >&2
    exit 1
fi
wait "$run_pid" 2>/dev/null || true
assert_removed "$interrupt_project" "an interrupt"

printf 'health-plane certification cleanup test: passed\n'
