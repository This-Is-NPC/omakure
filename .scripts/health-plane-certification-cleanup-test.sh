#!/usr/bin/env bash
set -euo pipefail

# Cleanup is verified for three shapes, all bounded:
#
#   1. partial startup - Compose creates the first node's resources and then a
#      sidecar exits non-zero under --abort-on-container-exit;
#   2. failure - the certification script exits non-zero from that point;
#   3. interrupt - the same run is signalled part-way through and must still
#      remove everything through its EXIT/INT/TERM trap, and must not report
#      success.
#
# The interrupt is SIGTERM, not SIGINT, and that is load-bearing. The gate is
# started here with `&` from a non-interactive shell, and bash forces SIGINT to
# ignored for asynchronous commands when job control is off. A signal ignored at
# exec cannot be trapped, so the gate's INT trap would never fire and the run
# would sail on to completion and exit 0 - which is exactly what this shape is
# supposed to catch. /proc/<pid>/status confirms it: SIGINT in SigIgn, only
# SIGTERM in SigCgt.
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
    timeout --foreground --kill-after=900s 20m "$root_dir/.scripts/health-plane-certification.sh"; then
    printf 'health-plane certification cleanup test: induced failure unexpectedly passed\n' >&2
    exit 1
fi
assert_removed "$induced_project" "an induced partial startup and failure"

# 3: interrupt part-way through a real run.
interrupt_project="omakure-health-plane-certification-interrupt-${BASHPID}"
OMAKURE_HEALTH_CERTIFICATION_PROJECT="$interrupt_project" \
    timeout --foreground --kill-after=1200s 20m "$root_dir/.scripts/health-plane-certification.sh" \
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
    kill -TERM "$run_pid" 2>/dev/null || true
    wait "$run_pid" 2>/dev/null || true
    printf 'health-plane certification cleanup test: the interrupt run never started containers\n' >&2
    exit 1
fi
kill -TERM "$run_pid" 2>/dev/null || true
# `--kill-after` above is deliberately larger than this ceiling. GNU timeout
# arms alarm(kill_after) whenever it *forwards* a signal, not only when its own
# limit expires, so a small value silently caps how long the gate gets to tear
# down: at `--kill-after=10s` the gate was SIGKILLed 10s after the TERM, mid
# `cleanup()`, and the parent saw 137. That was invisible while this test sent
# SIGINT, because SIGINT was never forwarded and no alarm was ever armed.
#
# Bash runs a trap only between commands, so a signal that lands while the gate
# is inside a bounded Docker command is not observed until that command
# returns. This test signals as soon as the first containers exist, which is
# Phase 1, where the in-flight command is a `compose run` bounded at 180s. The
# gate does have longer bounded commands elsewhere - the adversary matrix runs
# under 20m - but they cannot be in flight at this signal point.
#
#    180  the deferred Phase 1 Compose command
#    180  `compose logs`, which always runs because the signal exits non-zero
#    180  `compose down --volumes`
#    120  the reclaim `docker run` that hands host-side state back
#    360  three resource sweeps at 120s each
#   ----
#   1020  worst case, rounded to 1080 for headroom
#
# It cannot mask a hang, because every teardown command carries its own
# `timeout`; the loop breaks as soon as the process is gone.
#
# Observed, for calibration: with a signal that actually reaches the gate the
# whole three-shape run finishes in ~71s. A ceiling anywhere near that would be
# tuned to the happy path, so the worst case above is what is encoded. If this
# ever approaches the ceiling, the teardown regressed - do not just raise it.
interrupt_deadline=$((SECONDS + 1080))
while (( SECONDS < interrupt_deadline )); do
    kill -0 "$run_pid" 2>/dev/null || break
    sleep 2
done
if kill -0 "$run_pid" 2>/dev/null; then
    kill -TERM "$run_pid" 2>/dev/null || true
    wait "$run_pid" 2>/dev/null || true
    printf 'health-plane certification cleanup test: the signalled run did not exit within its bound\n' >&2
    exit 1
fi
# The gate's own contract is that an interrupt is never reported as success
# (`on_signal` exits 130). Assert it, or the shape only proves teardown.
interrupt_status=0
wait "$run_pid" 2>/dev/null || interrupt_status=$?
if (( interrupt_status == 0 )); then
    printf 'health-plane certification cleanup test: the signalled run reported success\n' >&2
    exit 1
fi
assert_removed "$interrupt_project" "an interrupt"

printf 'health-plane certification cleanup test: passed\n'
