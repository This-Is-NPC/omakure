#!/usr/bin/env bash
set -euo pipefail

# The one bounded Linux Health Plane certification command.
#
# Four independently stateful packaged nodes on one dedicated network prove the
# whole Health Plane through production paths only:
#
#   * every node-to-node Health Plane message crosses the shipped Noise
#     transport on port 7879. Management HTTP binds 127.0.0.1 inside each
#     container and is never published, so it is structurally incapable of
#     being the node-to-node data path; it is a read surface only.
#   * every adversarial case is injected over a real production Noise session
#     by `tests/docker_health_plane_adversary.rs`, never by a mock. That harness
#     dials into the published listeners from the host, which needs no inbound
#     host reachability. The attempt-exhaustion harness is the one case where
#     the Performer must be the initiator, so it runs as a container on the
#     dedicated network instead; no phase requires container-to-host traffic.
#   * every wait, retry, Docker command, curl, and sqlite query is explicitly
#     bounded, and teardown verifies that no container, network, or volume of
#     this project survives.
#
# Fleet roles are assigned at runtime from the freshly generated canonical node
# IDs. The shipped transport resolves dial ownership deterministically (the
# lower ID dials the higher one), so ranking the IDs is what makes the gate
# deterministic rather than a coin flip. See the Compose file for the ranking.

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
compose_file="$root_dir/compose.health-plane-certification.e2e.yaml"
project="${OMAKURE_HEALTH_CERTIFICATION_PROJECT:-omakure-health-plane-certification-${BASHPID}}"
image="omakure-node:health-plane-certification"
harness_image="omakure-node-harness:health-plane-certification"
compose=(timeout --foreground --kill-after=5s 180s docker compose -f "$compose_file" -p "$project")
docker_cmd=(timeout --foreground --kill-after=5s 120s docker)
sqlite=(timeout --foreground --kill-after=2s 30s sqlite3)
tmp_dir=$(mktemp -d)
# The attempt-exhaustion harness bind-mounts this directory and runs as this
# uid. Both are exported before the first Compose call so the service can guard
# them with `:?` instead of defaulting to root with a host path mounted in.
harness_dir="$tmp_dir/harness"
export OMAKURE_HP_HARNESS_DIR="$harness_dir"
export OMAKURE_HP_HARNESS_USER="$(id -u):$(id -g)"
induced_failure=${OMAKURE_HEALTH_CERTIFICATION_INDUCE_FAILURE:-0}

# ---------------------------------------------------------------------------
# Frozen bounds. Every number below is transcribed from
# `.docs/health-plane-contract.md` and `tests/fixtures/health_plane_vectors.toml`.
# None of them is chosen here.
# ---------------------------------------------------------------------------
FROZEN_SIGNAL_INBOX_CAPACITY=64
FROZEN_SIGNAL_RETENTION_SECONDS=604800
FROZEN_MAX_AUDIT_ROWS=10000
FROZEN_MAX_REPLAY_ROWS=131072
FROZEN_STORAGE_CEILING_BYTES=25464832
FROZEN_MAX_STORED_PROFILE_BYTES=2112
FROZEN_MAX_STORED_PULSE_BYTES=1344
FROZEN_MAX_STORED_SIGNAL_BYTES=1088
FROZEN_CAPABILITY_PROFILE_PULSE="inventory-health"
FROZEN_CAPABILITY_SIGNAL="notifications"

# ---------------------------------------------------------------------------
# Bounded wait budgets. Each is a real-time ceiling, never an unbounded poll.
# ---------------------------------------------------------------------------
# Service readiness on a shared CI host.
READY_BUDGET=90
# One node reaching `connected` with an expected peer.
CONNECT_BUDGET=90
# A Profile/Pulse reaching the Conductor. Strictly inside the frozen 120 s
# freshness window the gate then asserts against.
REPORT_BUDGET=110
# `online` -> `stale` after isolation. The frozen boundary is 91 s after the
# last accepted Pulse, so this budget is that boundary plus bounded slack.
STALE_BUDGET=200
# One retention/maintenance pass, whose cadence is the frozen 60 s rate window.
MAINTENANCE_BUDGET=180
# A durable audit row appearing after an injected case.
AUDIT_BUDGET=30
# Identity draws for the replacement identity, which must keep sorting below
# the Conductor for the shipped dial-ownership rule to hold.
MAX_IDENTITY_DRAWS=12

fail() {
    printf 'health-plane certification: %s\n' "$*" >&2
    exit 1
}

step() {
    printf 'health-plane certification: %s\n' "$*"
}

# A zero exit is not proof that anything ran. libtest exits 0 when its filter
# matches no test, so a renamed test - or one that loses `#[ignore]`, which
# makes `--ignored` match nothing - would leave its phase green having
# certified nothing at all. Both Noise harnesses are a single test invoked
# through a name that lives outside the Rust source (a Compose image CMD and a
# `--test` argument), so both must prove the test actually ran.
assert_single_test_ran() {
    local output=$1 what=$2
    printf '%s\n' "$output" | grep -q 'test result: ok\. 1 passed' && return 0
    printf '%s\n' "$output" >&2
    fail "$what reported success without running its test"
}

cleanup() {
    local exit_status=$?
    trap - EXIT INT TERM
    if (( exit_status != 0 )); then
        "${compose[@]}" logs --no-log-prefix hp-node-1 hp-node-2 hp-node-3 hp-node-4 >&2 || true
    fi
    if ! "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1; then
        printf 'health-plane certification: Compose teardown failed for %s\n' "$project" >&2
        exit_status=1
    fi
    # Any host-side copy of node state is written by the container's uid; give
    # it back before removing it so cleanup itself cannot fail on permissions.
    if [[ -d "$tmp_dir" ]]; then
        "${docker_cmd[@]}" run --rm --user 0 -v "$tmp_dir:/reclaim" \
            --entrypoint /usr/bin/chown "$image" \
            -R "$(id -u):$(id -g)" /reclaim >/dev/null 2>&1 || true
    fi
    for resource in container network volume; do
        local remaining
        if ! remaining=$("${docker_cmd[@]}" "$resource" ls -q \
            --filter "label=com.docker.compose.project=$project"); then
            printf 'health-plane certification: unable to inspect remaining %s resources for %s\n' \
                "$resource" "$project" >&2
            exit_status=1
        elif [[ -n "$remaining" ]]; then
            printf 'health-plane certification: project %s resources survived teardown for %s\n' \
                "$resource" "$project" >&2
            exit_status=1
        fi
    done
    rm -rf "$tmp_dir"
    exit "$exit_status"
}
# An interrupt must never be reported as success: the handler exits with the
# conventional 128+signal status, which then runs the EXIT trap exactly once.
#
# Caveat worth knowing before relying on the INT half. A shell cannot trap a
# signal that was ignored when it was exec'd, and bash forces SIGINT (and
# SIGQUIT) to ignored for asynchronous commands when job control is off. So a
# runner that launches this script with `&` from a non-interactive shell gets a
# process whose /proc status shows SIGINT in SigIgn and only SIGTERM in SigCgt:
# the INT trap below silently does nothing. Terminal Ctrl+C is unaffected, and
# SIGTERM always works. Automation should send SIGTERM.
on_signal() {
    local signal=$1
    trap - INT TERM
    printf 'health-plane certification: interrupted by SIG%s; tearing down\n' "$signal" >&2
    case "$signal" in
        INT) exit 130 ;;
        TERM) exit 143 ;;
        *) exit 1 ;;
    esac
}
trap cleanup EXIT
trap 'on_signal INT' INT
trap 'on_signal TERM' TERM

command -v docker >/dev/null || fail "docker is required"
command -v jq >/dev/null || fail "jq is required"
command -v sqlite3 >/dev/null || fail "sqlite3 is required for durable audit inspection"
command -v cargo >/dev/null || fail "cargo is required for the production Noise harnesses"
"${docker_cmd[@]}" compose version >/dev/null || fail "Docker Compose is required"

# ---------------------------------------------------------------------------
# Authentication material for the management read surfaces.
# ---------------------------------------------------------------------------
generate_auth() {
    local service=$1
    local tokens_path="$tmp_dir/${service}.tokens.toml"
    local client_path="$tmp_dir/${service}.client.token"
    local curl_path="$tmp_dir/${service}.curl.conf"
    local stderr_path="$tmp_dir/${service}.token-generation.stderr"
    shift
    local token_json status
    if token_json=$("${docker_cmd[@]}" run --rm --entrypoint /usr/local/bin/omakure "$image" \
        --json token generate --id "health-$service" "$@" 2>"$stderr_path"); then
        :
    else
        status=$?
        generation_failure "$service" "$status" "$stderr_path"
    fi
    printf 'version = 1\n\n' >"$tokens_path"
    jq -er '.data.tokens_file_entry' <<<"$token_json" >>"$tokens_path" \
        || fail "token entry generation failed for $service"
    jq -er '.data.token' <<<"$token_json" >"$client_path" \
        || fail "client token generation failed for $service"
    printf 'header = "Authorization: Bearer %s"\n' "$(<"$client_path")" >"$curl_path"
    chmod 0600 "$tokens_path" "$client_path" "$curl_path"
    printf '%s\n' "$tokens_path|$client_path|$curl_path"
}

generation_failure() {
    local service=$1 status=$2 stderr_path=$3
    local stderr
    stderr=$(<"$stderr_path")
    case "${stderr,,}" in
        *"bearer "*|*'$argon2'*|*"token ="*)
            fail "token generation failed for $service: status=$status; stderr contained sensitive material"
            ;;
    esac
    fail "token generation failed for $service: status=$status; stderr=$stderr"
}

# ---------------------------------------------------------------------------
# Bounded node/Compose helpers.
# ---------------------------------------------------------------------------
last_json_line() {
    local line last=''
    while IFS= read -r line; do
        if jq -e . <<<"$line" >/dev/null 2>&1; then
            last=$line
        fi
    done
    [[ -n "$last" ]] || fail "command did not produce a JSON envelope"
    printf '%s\n' "$last"
}

# Run one node command in a throwaway container over the service's volumes.
run_node() {
    local service=$1
    shift
    local output
    output=$("${compose[@]}" run --rm --no-deps -T "$service" --json node "$@")
    last_json_line <<<"$output"
}

# Run one node command inside the live service container.
exec_node() {
    local service=$1
    shift
    local output
    output=$("${compose[@]}" exec -T "$service" /usr/local/bin/omakure --json node "$@")
    last_json_line <<<"$output"
}

exec_cli() {
    local service=$1
    shift
    local output
    output=$("${compose[@]}" exec -T "$service" /usr/local/bin/omakure --json "$@")
    last_json_line <<<"$output"
}

write_config() {
    local service=$1 config=$2
    printf '%s\n' "$config" | "${compose[@]}" run --rm --no-deps -T --user 0:0 \
        --entrypoint /bin/sh "$service" \
        -c 'cat > /etc/omakure/node.toml && chown root:10001 /etc/omakure/node.toml && chmod 0640 /etc/omakure/node.toml'
}

node_id() { jq -er '.data.identity.node_id'; }
node_key() { jq -er '.data.identity.public_key'; }

certificate() {
    local service=$1
    "${compose[@]}" run --rm --no-deps -T --entrypoint /bin/sh "$service" -c \
        'od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d " \n"'
}

persisted_material_hashes() {
    local service=$1
    "${compose[@]}" exec -T --user 0:0 "$service" /bin/sh -c '
        for path in /var/lib/omakure/identity.key /var/lib/omakure/transport.key /var/lib/omakure/transport.cert; do
            test -f "$path"
            sha256sum "$path" | cut -d " " -f1
        done
    '
}

http_get() {
    local service=$1 path=$2
    "${compose[@]}" exec -T "$service" curl --config /run/secrets/curl.conf \
        --connect-timeout 3 --max-time 10 -fsS "http://127.0.0.1:7878$path"
}

# Copy the registry *and* its write-ahead log, so a durable row committed a
# moment ago is visible to the host-side reader instead of being missed.
copy_db() {
    local service=$1 destination=$2
    rm -f "$destination" "$destination-wal" "$destination-shm"
    "${compose[@]}" cp "$service:/var/lib/omakure/node.sqlite" "$destination" >/dev/null \
        || fail "copying $service registry failed"
    "${compose[@]}" cp "$service:/var/lib/omakure/node.sqlite-wal" "$destination-wal" \
        >/dev/null 2>&1 || true
}

sql() {
    local database=$1
    shift
    "${sqlite[@]}" "$database" "$@"
}

# Resolve one service's address on the dedicated network. The production
# `direct-probe` verb takes a socket address, never a hostname.
peer_ip() {
    local service=$1 resolver=$2 address
    address=$("${compose[@]}" run --rm --no-deps -T --entrypoint /bin/sh "$resolver" -c \
        "getent ahostsv4 $service | cut -d' ' -f1 | sort -u | head -n 1" | tr -d '\r\n')
    [[ -n "$address" ]] || fail "unable to resolve $service on the dedicated network"
    printf '%s\n' "$address"
}

container_id() {
    local service=$1 id
    id=$("${compose[@]}" ps -q "$service") || fail "unable to resolve container for $service"
    [[ -n "$id" ]] || fail "no running container for $service"
    printf '%s\n' "$id"
}

# ---------------------------------------------------------------------------
# Bounded waits. Every one of them fails closed at its deadline.
# ---------------------------------------------------------------------------
wait_service() {
    local service=$1 deadline=$((SECONDS + READY_BUDGET))
    while (( SECONDS < deadline )); do
        if "${compose[@]}" exec -T "$service" /usr/local/bin/omakure --json node status \
            >/dev/null 2>&1; then
            return
        fi
        sleep 1
    done
    fail "$service did not reach bounded status readiness within ${READY_BUDGET}s"
}

wait_connected() {
    local service=$1 peer_id=$2 output='' deadline=$((SECONDS + CONNECT_BUDGET))
    while (( SECONDS < deadline )); do
        if output=$(http_get "$service" /v1/node/status 2>/dev/null) \
            && jq -e --arg peer "$peer_id" \
                '.data.transport.peers | any(.[]; .node_id == $peer and .state == "connected")' \
                <<<"$output" >/dev/null; then
            return
        fi
        sleep 1
    done
    fail "$service did not connect to $peer_id within ${CONNECT_BUDGET}s; last status=${output:-<none>}"
}

fleet_json() { http_get "$1" /v1/node/health; }
signals_json() { http_get "$1" /v1/node/signals; }

presence_of() {
    local fleet=$1 peer=$2
    jq -r --arg peer "$peer" \
        'first(.data.nodes[] | select(.node_id == $peer) | .presence) // "absent"' <<<"$fleet"
}

wait_presence() {
    local service=$1 peer=$2 expected=$3 budget=$4
    local deadline=$((SECONDS + budget)) fleet='' observed=''
    while (( SECONDS < deadline )); do
        if fleet=$(fleet_json "$service" 2>/dev/null); then
            observed=$(presence_of "$fleet" "$peer")
            if [[ "$observed" == "$expected" ]]; then
                printf '%s\n' "$fleet"
                return
            fi
        fi
        sleep 2
    done
    fail "$service never reported $peer as $expected within ${budget}s; last presence=${observed:-<none>}"
}

wait_signal_count() {
    local service=$1 source=$2 kind=$3 expected=$4 budget=$5
    local deadline=$((SECONDS + budget)) feed='' observed=''
    while (( SECONDS < deadline )); do
        if feed=$(signals_json "$service" 2>/dev/null); then
            observed=$(jq -r --arg src "$source" --arg kind "$kind" \
                '[.data.signals[] | select(.source == $src and .kind == $kind)] | length' <<<"$feed")
            if [[ "$observed" == "$expected" ]]; then
                printf '%s\n' "$feed"
                return
            fi
        fi
        sleep 2
    done
    fail "$service never reported $expected '$kind' Signal(s) from $source within ${budget}s; last count=${observed:-<none>}"
}

wait_health_audit_code() {
    local service=$1 after_id=$2 expected_code=$3
    local deadline=$((SECONDS + AUDIT_BUDGET)) row='' database="$tmp_dir/${service}.audit.sqlite"
    while (( SECONDS < deadline )); do
        copy_db "$service" "$database"
        row=$(sql "$database" -separator '|' \
            "SELECT id, event_code, node_id, message_kind, byte_count, outcome, COALESCE(error_code, '')
             FROM health_audit WHERE id > $after_id AND error_code = $expected_code ORDER BY id LIMIT 1;")
        if [[ -n "$row" ]]; then
            printf '%s\n' "$row"
            return
        fi
        sleep 1
    done
    fail "$service recorded no durable Health Plane audit row with code $expected_code after id $after_id within ${AUDIT_BUDGET}s"
}

# ---------------------------------------------------------------------------
# Durable-state snapshots. Everything an adversary must never be able to touch.
# ---------------------------------------------------------------------------
peer_trust_snapshot() {
    local database=$1 peer=$2
    sql "$database" -separator '|' \
        "SELECT
           COALESCE((SELECT state || ':' || hex(identity_key) FROM remote_identities WHERE node_id = '$peer'), 'none'),
           COALESCE((SELECT state || ':' || role || ':' || hex(capabilities) FROM trusted_peers WHERE node_id = '$peer'), 'none'),
           (SELECT COUNT(*) FROM revocations WHERE node_id = '$peer');"
}

peer_health_snapshot() {
    local database=$1 peer=$2
    sql "$database" -separator '|' \
        "SELECT
           COALESCE((SELECT role || ':' || cursor FROM health_peers WHERE node_id = '$peer'), 'none'),
           (SELECT COUNT(*) FROM health_profiles WHERE node_id = '$peer'),
           (SELECT COUNT(*) FROM health_pulses WHERE node_id = '$peer'),
           COALESCE((SELECT group_concat(value, ';') FROM (SELECT hex(signal_id) || ':' || sequence || ':' || state AS value FROM health_signals WHERE node_id = '$peer' ORDER BY sequence)), '');"
}

latest_health_audit_id() {
    sql "$1" "SELECT COALESCE(MAX(id), 0) FROM health_audit;"
}

health_storage_bytes() {
    sql "$1" <<'SQL'
SELECT
  (SELECT COALESCE(SUM(message_bytes), 0) FROM health_profiles)
+ (SELECT COALESCE(SUM(message_bytes), 0) FROM health_pulses)
+ (SELECT COALESCE(SUM(message_bytes), 0) FROM health_signals)
+ (SELECT COUNT(*) FROM health_replay_keys) * 32
+ (SELECT COUNT(*) FROM health_audit) * 256;
SQL
}

assert_frozen_storage_bounds() {
    local database=$1 label=$2
    local bytes audit_rows replay_rows over_kind over_inbox duplicate_latest
    bytes=$(health_storage_bytes "$database")
    (( bytes <= FROZEN_STORAGE_CEILING_BYTES )) \
        || fail "$label exceeded the frozen Health Plane storage ceiling: $bytes > $FROZEN_STORAGE_CEILING_BYTES"
    audit_rows=$(sql "$database" "SELECT COUNT(*) FROM health_audit;")
    (( audit_rows <= FROZEN_MAX_AUDIT_ROWS )) \
        || fail "$label exceeded the frozen Health Plane audit row cap: $audit_rows > $FROZEN_MAX_AUDIT_ROWS"
    replay_rows=$(sql "$database" "SELECT COUNT(*) FROM health_replay_keys;")
    (( replay_rows <= FROZEN_MAX_REPLAY_ROWS )) \
        || fail "$label exceeded the frozen replay row cap: $replay_rows > $FROZEN_MAX_REPLAY_ROWS"
    over_kind=$(sql "$database" \
        "SELECT (SELECT COUNT(*) FROM health_profiles WHERE message_bytes > $FROZEN_MAX_STORED_PROFILE_BYTES)
              + (SELECT COUNT(*) FROM health_pulses WHERE message_bytes > $FROZEN_MAX_STORED_PULSE_BYTES)
              + (SELECT COUNT(*) FROM health_signals WHERE message_bytes > $FROZEN_MAX_STORED_SIGNAL_BYTES);")
    (( over_kind == 0 )) || fail "$label stored a row past its frozen per-kind byte cap"
    over_inbox=$(sql "$database" \
        "SELECT COUNT(*) FROM (SELECT node_id FROM health_signals GROUP BY node_id HAVING COUNT(*) > $FROZEN_SIGNAL_INBOX_CAPACITY);")
    (( over_inbox == 0 )) \
        || fail "$label stored more than the frozen $FROZEN_SIGNAL_INBOX_CAPACITY Signals for one Performer"
    duplicate_latest=$(sql "$database" \
        "SELECT (SELECT COUNT(*) FROM (SELECT node_id FROM health_profiles GROUP BY node_id HAVING COUNT(*) > 1))
              + (SELECT COUNT(*) FROM (SELECT node_id FROM health_pulses GROUP BY node_id HAVING COUNT(*) > 1));")
    (( duplicate_latest == 0 )) \
        || fail "$label retained more than the frozen one Profile/Pulse row per Performer"
}

assert_redacted_health_audit() {
    local database=$1 audit bad_outcome
    audit=$(sql "$database" \
        "SELECT event_code || '|' || node_id || '|' || message_kind || '|' || outcome || '|' || COALESCE(error_code, '') FROM health_audit ORDER BY id;")
    [[ -n "$audit" ]] || fail "the Health Plane audit trail is empty"
    [[ "$audit" != *"health-hp-node"* ]] || fail "a management token id leaked into the Health Plane audit"
    [[ "$audit" != *"/"* ]] || fail "a path fragment leaked into the Health Plane audit"
    [[ "$audit" != *"7879"* ]] || fail "an endpoint leaked into the Health Plane audit"
    bad_outcome=$(sql "$database" \
        "SELECT COUNT(*) FROM health_audit WHERE outcome NOT IN ('accepted','held','rejected','dropped','purged');")
    (( bad_outcome == 0 )) || fail "the Health Plane audit recorded an outcome outside the frozen set"
}

# Privacy class P1 must never appear in a public read surface.
assert_no_p1_leakage() {
    local label=$1 payload=$2 forbidden
    for forbidden in "/workspace" "/var/lib/omakure" "/etc/omakure" ".sh" "stdout" "stderr" \
        "secret://" "Bearer " '$argon2' "hostname" "ip_address" "mac_address"; do
        [[ "$payload" != *"$forbidden"* ]] || fail "$label leaked privacy class P1 marker '$forbidden'"
    done
}

assert_no_log_leakage() {
    local logs token_file token
    # hp-harness is included: it holds real node material and its panics are
    # forwarded to stderr, so it is exactly as capable of leaking as a node.
    # Fail closed. This is the run's only leakage gate, and `|| true` on the
    # capture made it fail *open*: if the Compose call itself failed, `$logs`
    # held the error text and every check below passed having scanned nothing.
    logs=$("${compose[@]}" logs --no-log-prefix \
        hp-node-1 hp-node-2 hp-node-3 hp-node-4 hp-harness 2>&1) \
        || fail "unable to read Compose logs for the leakage scan"
    # Positive control: the harness always announces the connection it accepted,
    # so its absence means the capture is not the logs it is supposed to be.
    [[ "$logs" == *"harness: accepted a connection"* ]] \
        || fail "the leakage scan captured no harness output; it scanned the wrong thing"
    [[ "$logs" != *"Bearer "* ]] || fail "bearer value leaked into Compose logs"
    [[ "$logs" != *'$argon2'* ]] || fail "Argon2 token hash leaked into Compose logs"
    for token_file in "$tmp_dir"/*.client.token; do
        token=$(<"$token_file")
        [[ "$logs" != *"$token"* ]] || fail "bearer value from $token_file leaked into Compose logs"
    done
}

# Re-establish both Performers' sessions with the Conductor.
#
# This is shipped transport behaviour, not a workaround: `DirectService`'s
# static-peer dialer stops after the frozen `MAX_RETRY_ATTEMPTS` consecutive
# connection failures (`src/direct_service.rs`), so a Performer that outlived a
# Conductor outage longer than its frozen 1/2/4-second backoff needs an explicit
# restart to dial again. The existing transport certification restarts peers for
# the same reason. Every step below is bounded.
restore_fleet_sessions() {
    local performer
    "${compose[@]}" restart "$alpha_svc" "$beta_svc" >/dev/null \
        || fail "restarting the Performers after a Conductor outage failed"
    for performer in "$alpha_svc" "$beta_svc"; do
        wait_service "$performer"
    done
    wait_connected "$conductor_svc" "$alpha_id"
    wait_connected "$conductor_svc" "$beta_id"
}

reclaim_material() {
    local directory=$1
    "${docker_cmd[@]}" run --rm --user 0 -v "$directory:/material" \
        --entrypoint /usr/bin/chown "$image" -R "$(id -u):$(id -g)" /material >/dev/null \
        || fail "reclaiming ownership of $directory failed"
}

# ---------------------------------------------------------------------------
# Phase 0: build the current image and provision management authentication.
# ---------------------------------------------------------------------------
step 'build the current image'
timeout --foreground --kill-after=30s 25m \
    docker build --pull=false --target runtime --tag "$image" "$root_dir" >/dev/null \
    || fail "image build failed"
# The attempt-exhaustion harness ships as its own image so it can run on the
# certification network instead of on the host. It reuses the `builder` stage,
# so this adds a test-binary compile rather than a second full build.
step 'build the attempt-exhaustion harness image'
timeout --foreground --kill-after=30s 25m \
    docker build --pull=false --target harness --tag "$harness_image" "$root_dir" >/dev/null \
    || fail "attempt-exhaustion harness image build failed"

for index in 1 2 3 4; do
    auth=$(generate_auth "hp-node-$index" --scope node:read --scope enrollment:read --scope enrollment:write) \
        || fail "hp-node-$index auth setup failed"
    IFS='|' read -r tokens client curl_conf <<<"$auth"
    export "OMAKURE_HP_NODE_${index}_TOKENS_FILE=$tokens"
    export "OMAKURE_HP_NODE_${index}_CLIENT_FILE=$client"
    export "OMAKURE_HP_NODE_${index}_CURL_FILE=$curl_conf"
done
"${compose[@]}" build --pull=false >/dev/null || fail "compose build failed"

# ---------------------------------------------------------------------------
# Phase 1: independent identities, deterministic role assignment.
# ---------------------------------------------------------------------------
step 'initialize four independently stateful nodes'
for index in 1 2 3 4; do
    run_node "hp-node-$index" init >/dev/null || fail "hp-node-$index initialization failed"
done

declare -A service_of_id=()
ids=()
for index in 1 2 3 4; do
    status=$(run_node "hp-node-$index" status)
    id=$(node_id <<<"$status")
    service_of_id["$id"]="hp-node-$index"
    ids+=("$id")
done
mapfile -t ranked < <(printf '%s\n' "${ids[@]}" | LC_ALL=C sort -u)
(( ${#ranked[@]} == 4 )) || fail "expected four distinct node identities"

alpha_svc=${service_of_id[${ranked[0]}]}
beta_svc=${service_of_id[${ranked[1]}]}
adversary_svc=${service_of_id[${ranked[2]}]}
conductor_svc=${service_of_id[${ranked[3]}]}
alpha_id=${ranked[0]}
beta_id=${ranked[1]}
adversary_id=${ranked[2]}
conductor_id=${ranked[3]}
step "roles: conductor=$conductor_svc performers=$alpha_svc,$beta_svc adversary=$adversary_svc"

direct_port_of() {
    case "$1" in
        hp-node-1) printf '%s\n' "${OMAKURE_HP_NODE_1_DIRECT_PORT:-17901}" ;;
        hp-node-2) printf '%s\n' "${OMAKURE_HP_NODE_2_DIRECT_PORT:-17902}" ;;
        hp-node-3) printf '%s\n' "${OMAKURE_HP_NODE_3_DIRECT_PORT:-17903}" ;;
        hp-node-4) printf '%s\n' "${OMAKURE_HP_NODE_4_DIRECT_PORT:-17904}" ;;
        *) fail "unknown service $1" ;;
    esac
}

declare -A node_key_of=() node_cert_of=()
for svc in "$alpha_svc" "$beta_svc" "$adversary_svc" "$conductor_svc"; do
    status=$(run_node "$svc" status)
    node_key_of["$svc"]=$(node_key <<<"$status")
    node_cert_of["$svc"]=$(certificate "$svc")
done

static_config() {
    local display_name=$1 peers=$2
    cat <<EOF
version = 1

[node]
display_name = "$display_name"

[api]
bind = "127.0.0.1:7878"

[network]
mode = "direct"
relays = []
static_peers = [$peers]
direct_bind = "0.0.0.0:7879"
max_message_bytes = 1048576

[trust]
enrollment = "manual"
allow_remote_cues = false
allow_baseline_push = false

[organization]
id = "omakure-health-certification"
discovery_secret_ref = ""
EOF
}

write_config "$conductor_svc" \
    "$(static_config health-conductor "\"$alpha_id@$alpha_svc:7879\", \"$beta_id@$beta_svc:7879\"")"
write_config "$alpha_svc" \
    "$(static_config health-performer-alpha "\"$conductor_id@$conductor_svc:7879\"")"
write_config "$beta_svc" \
    "$(static_config health-performer-beta "\"$conductor_id@$conductor_svc:7879\"")"
write_config "$adversary_svc" "$(static_config health-adversary "")"

# ---------------------------------------------------------------------------
# Phase 2: explicit trust and capability provisioning.
# ---------------------------------------------------------------------------
step 'provision explicit trust, roles, and capabilities'
trust_offline() {
    local service=$1 peer_id=$2 peer_key=$3 peer_cert=$4 role=$5 reason=$6
    shift 6
    local args=(trust --node-id "$peer_id" --public-key "$peer_key"
        --transport-certificate "$peer_cert" --role "$role"
        --actor health-certification --reason "$reason" --confirmed)
    local capability
    for capability in "$@"; do
        args+=(--capability "$capability")
    done
    run_node "$service" "${args[@]}" >/dev/null \
        || fail "trusting $peer_id from $service as $role failed"
}

trust_offline "$conductor_svc" "$alpha_id" "${node_key_of[$alpha_svc]}" "${node_cert_of[$alpha_svc]}" \
    performer "certified performer alpha" "$FROZEN_CAPABILITY_PROFILE_PULSE" "$FROZEN_CAPABILITY_SIGNAL"
trust_offline "$conductor_svc" "$beta_id" "${node_key_of[$beta_svc]}" "${node_cert_of[$beta_svc]}" \
    performer "certified performer beta" "$FROZEN_CAPABILITY_PROFILE_PULSE" "$FROZEN_CAPABILITY_SIGNAL"
trust_offline "$alpha_svc" "$conductor_id" "${node_key_of[$conductor_svc]}" "${node_cert_of[$conductor_svc]}" \
    conductor "certified conductor" "$FROZEN_CAPABILITY_PROFILE_PULSE" "$FROZEN_CAPABILITY_SIGNAL"
trust_offline "$beta_svc" "$conductor_id" "${node_key_of[$conductor_svc]}" "${node_cert_of[$conductor_svc]}" \
    conductor "certified conductor" "$FROZEN_CAPABILITY_PROFILE_PULSE" "$FROZEN_CAPABILITY_SIGNAL"

if [[ "$induced_failure" == 1 ]]; then
    if "${compose[@]}" up --abort-on-container-exit --exit-code-from hp-induced-failure \
        hp-node-1 hp-induced-failure >/dev/null 2>&1; then
        fail "induced partial-up failure unexpectedly passed"
    fi
    fail "induced partial-up failure"
fi

# ---------------------------------------------------------------------------
# Phase 3: bring the fleet up over production Noise.
# ---------------------------------------------------------------------------
step 'start the fleet and reach steady state over production Noise'
"${compose[@]}" up -d hp-node-1 hp-node-2 hp-node-3 hp-node-4 >/dev/null || fail "compose up failed"
for svc in "$conductor_svc" "$alpha_svc" "$beta_svc" "$adversary_svc"; do
    wait_service "$svc"
done
wait_connected "$conductor_svc" "$alpha_id"
wait_connected "$conductor_svc" "$beta_id"
wait_connected "$alpha_svc" "$conductor_id"
wait_connected "$beta_svc" "$conductor_id"

# The topology's isolation is asserted, not assumed.
network_name="${project}_hp-net"
attached=$("${docker_cmd[@]}" network inspect "$network_name" \
    --format '{{range .Containers}}{{.Name}}{{"\n"}}{{end}}' | sed '/^$/d' | LC_ALL=C sort)
attached_count=$(printf '%s\n' "$attached" | sed '/^$/d' | wc -l)
(( attached_count == 4 )) \
    || fail "the dedicated network holds $attached_count containers, expected exactly the four nodes: $attached"
while IFS= read -r name; do
    [[ "$name" == "$project"-* ]] \
        || fail "a container outside this project is attached to the dedicated network: $name"
done <<<"$attached"

step 'prove management HTTP is a read surface only, never the data path'
for target in "$conductor_svc" "$alpha_svc" "$beta_svc"; do
    if "${compose[@]}" exec -T "$adversary_svc" curl --connect-timeout 3 --max-time 8 -sS \
        "http://$target:7878/v1/health" >/dev/null 2>&1; then
        fail "$target's management HTTP is reachable from another container"
    fi
done
# Only a published mapping matters here. The image's `EXPOSE 7878` is metadata
# and creates no host or peer reachability, which the peer-to-peer curl probes
# above already proved.
published=$("${docker_cmd[@]}" ps --filter "label=com.docker.compose.project=$project" \
    --format '{{.Ports}}')
[[ "$published" != *"->7878"* ]] \
    || fail "management HTTP is published outside the container: $published"
[[ "$published" == *"->7879"* ]] \
    || fail "the direct transport listener is not the published node-to-node port: $published"

# ---------------------------------------------------------------------------
# Phase 4: two Performers independently online, Profile + Pulse, both adapters.
# ---------------------------------------------------------------------------
step 'certify Profile, Pulse, fleet aggregation, and both read adapters'
wait_presence "$conductor_svc" "$alpha_id" online "$REPORT_BUDGET" >/dev/null
fleet=$(wait_presence "$conductor_svc" "$beta_id" online "$REPORT_BUDGET")

cli_fleet=$(exec_node "$conductor_svc" health)
http_stable=$(jq -Sc '[.data.nodes[] | {node_id, role, capabilities, trust_state, profile}]' <<<"$fleet")
cli_stable=$(jq -Sc '[.data.nodes[] | {node_id, role, capabilities, trust_state, profile}]' <<<"$cli_fleet")
[[ "$http_stable" == "$cli_stable" ]] \
    || fail "the CLI and HTTP fleet-status adapters disagree: cli=$cli_stable http=$http_stable"
[[ "$(jq -r '.data.local_node_id' <<<"$fleet")" == "$conductor_id" ]] \
    || fail "the fleet report is not anchored on the Conductor's own identity"
[[ "$(jq -r '.data.enabled' <<<"$fleet")" == "true" ]] || fail "the Health Plane is not enabled"
[[ "$(jq -r '.data.presence.online' <<<"$fleet")" == "2" ]] \
    || fail "expected exactly two online Performers: $(jq -c '.data.presence' <<<"$fleet")"
[[ "$(jq -r '.data.presence.total' <<<"$fleet")" == "2" ]] \
    || fail "the fleet is not exactly the two trusted Performers"

for peer in "$alpha_id" "$beta_id"; do
    node=$(jq -c --arg peer "$peer" 'first(.data.nodes[] | select(.node_id == $peer))' <<<"$fleet")
    [[ "$(jq -r '.role' <<<"$node")" == "performer" ]] || fail "$peer is not projected as a Performer"
    [[ "$(jq -r '.trust_state' <<<"$node")" == "active" ]] || fail "$peer is not actively trusted"
    [[ "$(jq -c '.capabilities' <<<"$node")" == "[\"$FROZEN_CAPABILITY_PROFILE_PULSE\",\"$FROZEN_CAPABILITY_SIGNAL\"]" ]] \
        || fail "$peer does not carry exactly the two frozen Health Plane capabilities"
    [[ "$(jq -r '.profile.platform' <<<"$node")" == "linux" ]] || fail "$peer reported a wrong platform"
    [[ "$(jq -r '.profile.role' <<<"$node")" == "performer" ]] || fail "$peer reported a wrong Profile role"
    (( $(jq -r '.profile.profile_revision' <<<"$node") >= 1 )) || fail "$peer reported no Profile revision"
    (( $(jq -r '.pulse.sequence' <<<"$node") >= 1 )) || fail "$peer reported no Pulse sequence"
    [[ "$(jq -r '.pulse.runner.scheduler' <<<"$node")" == "disabled" ]] \
        || fail "$peer reported a scheduler state that does not match its packaged configuration"
    [[ "$(jq -r '.version_incompatible' <<<"$node")" == "false" ]] || fail "$peer is version-incompatible"
done
[[ "$(jq -r --arg a "$alpha_id" \
    'first(.data.nodes[] | select(.node_id == $a) | .profile.display_name)' <<<"$fleet")" == "health-performer-alpha" ]] \
    || fail "performer alpha did not report its own configured display name"
[[ "$(jq -r --arg b "$beta_id" \
    'first(.data.nodes[] | select(.node_id == $b) | .profile.display_name)' <<<"$fleet")" == "health-performer-beta" ]] \
    || fail "performer beta did not report its own configured display name"
assert_no_p1_leakage "the fleet-status projection" "$fleet"

copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
accepted_health=$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COUNT(*) FROM health_audit WHERE outcome = 'accepted' AND message_kind IN ('health_profile','health_pulse');")
(( accepted_health >= 4 )) \
    || fail "expected at least four accepted Health Plane messages (Profile + Pulse per Performer), got $accepted_health"
accepted_sessions=$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COUNT(DISTINCT node_id) FROM transport_audit WHERE outcome = 'accepted' AND session_id IS NOT NULL;")
(( accepted_sessions >= 2 )) \
    || fail "expected accepted encrypted sessions with both Performers, got $accepted_sessions"
assert_frozen_storage_bounds "$tmp_dir/conductor.sqlite" "the Conductor after steady state"

# ---------------------------------------------------------------------------
# Phase 5: the closed Signal lifecycle - enrolled, run-completed, revoked.
# ---------------------------------------------------------------------------
step 'certify the enrolled Signals both Performers produced at trust time'
feed=$(signals_json "$conductor_svc")
enrolled=$(jq -c '[.data.signals[] | select(.source == "local" and .kind == "enrolled") | .subject] | sort' <<<"$feed")
[[ "$enrolled" == "$(jq -nc --arg a "$alpha_id" --arg b "$beta_id" '[$a, $b] | sort')" ]] \
    || fail "expected exactly one enrolled Signal per Performer, got $enrolled"
[[ "$(jq -r '.data.gap' <<<"$feed")" == "false" ]] || fail "the Signal feed is stalled behind a gap"
[[ "$(jq -r '.data.retention_seconds' <<<"$feed")" == "$FROZEN_SIGNAL_RETENTION_SECONDS" ]] \
    || fail "the Signal feed does not advertise the frozen 7-day retention window"
[[ "$(jq -r '.data.limit' <<<"$feed")" == "$FROZEN_SIGNAL_INBOX_CAPACITY" ]] \
    || fail "the Signal feed does not advertise the frozen inbox bound"

step 'certify one idempotent run-completed Signal from a real manual run'
# A manual `omakure run` is deliberately used rather than a scheduled run: the
# scheduler is the only write site that records `runs.script_name`, so the
# manual path is the one that exercises the script-stem fallback end to end.
schema='{"Name":"deploy","Description":"health plane certification","Fields":[]}'
exec_cli "$alpha_svc" init tools/deploy.sh --schema-json "$schema" >/dev/null \
    || fail "installing the certification script on performer alpha failed"
run_json=$(exec_cli "$alpha_svc" run tools/deploy.sh) || fail "the manual run on performer alpha failed"
local_run_id=$(jq -er '.data.run_id' <<<"$run_json") || fail "the manual run reported no run id"

feed=$(wait_signal_count "$conductor_svc" "$alpha_id" run-completed 1 "$REPORT_BUDGET")
signal=$(jq -c --arg src "$alpha_id" \
    'first(.data.signals[] | select(.source == $src and .kind == "run-completed"))' <<<"$feed")
[[ "$(jq -r '.sequence' <<<"$signal")" == "1" ]] || fail "the first Signal did not start the cursor at 1"
[[ "$(jq -r '.subject' <<<"$signal")" == "null" ]] || fail "a run-completed Signal must carry no subject"
[[ "$(jq -r '.run | length' <<<"$signal")" == "5" ]] \
    || fail "the frozen Signal run object must have exactly five fields: $signal"
[[ "$(jq -r '.run.script' <<<"$signal")" == "deploy" ]] \
    || fail "the Signal did not carry the script schema name: $signal"
[[ "$(jq -r '.run.state' <<<"$signal")" == "completed" ]] || fail "the Signal reported a wrong run state"
[[ "$(jq -r '.run.exit_code' <<<"$signal")" == "0" ]] || fail "the Signal reported a wrong exit code"
opaque_run_id=$(jq -r '.run.run_id' <<<"$signal")
[[ "$opaque_run_id" =~ ^[0-9a-f]{32}$ ]] || fail "the wire run id is not a 32-character opaque identifier"
[[ "$opaque_run_id" != "$local_run_id" ]] || fail "the local run id crossed the Health Plane boundary"
[[ "$feed" != *"$local_run_id"* ]] || fail "the local run id leaked into the Signal feed"
[[ "$feed" != *"tools/deploy"* ]] || fail "the script path leaked into the Signal feed"
assert_no_p1_leakage "the Signal feed" "$feed"
cli_feed=$(exec_node "$conductor_svc" signals)
[[ "$(jq -Sc '[.data.signals[] | {source, kind, sequence, signal_id}]' <<<"$feed")" \
    == "$(jq -Sc '[.data.signals[] | {source, kind, sequence, signal_id}]' <<<"$cli_feed")" ]] \
    || fail "the CLI and HTTP Signal adapters disagree"

step 'certify Signal idempotency across a real Performer restart'
"${compose[@]}" restart "$alpha_svc" >/dev/null || fail "restarting performer alpha failed"
wait_service "$alpha_svc"
wait_connected "$conductor_svc" "$alpha_id"
wait_presence "$conductor_svc" "$alpha_id" online "$REPORT_BUDGET" >/dev/null
feed=$(signals_json "$conductor_svc")
repeated=$(jq -r --arg src "$alpha_id" \
    '[.data.signals[] | select(.source == $src and .kind == "run-completed")] | length' <<<"$feed")
[[ "$repeated" == "1" ]] || fail "a Performer restart duplicated the run-completed Signal: $repeated"
cursor=$(jq -c --arg src "$alpha_id" 'first(.data.cursors[] | select(.node_id == $src))' <<<"$feed")
[[ "$(jq -r '.cursor' <<<"$cursor")" == "1" ]] || fail "the Signal cursor moved past one Signal: $cursor"
[[ "$(jq -r '.stored' <<<"$cursor")" == "1" ]] || fail "the Conductor stored more than one Signal: $cursor"
[[ "$(jq -r '.held' <<<"$cursor")" == "0" ]] || fail "the reorder buffer is holding a Signal: $cursor"
[[ "$(jq -r '.gap' <<<"$cursor")" == "false" ]] || fail "the Signal feed stalled behind a gap: $cursor"

# ---------------------------------------------------------------------------
# Phase 6: isolation, staleness, and recovery with fresh state.
# ---------------------------------------------------------------------------
step 'certify online -> stale -> recovery across a real network partition'
beta_container=$(container_id "$beta_svc")
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
beta_sequence_before=$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE((SELECT sequence FROM health_pulses WHERE node_id = '$beta_id'), 0);")
"${docker_cmd[@]}" network disconnect "$network_name" "$beta_container" \
    || fail "isolating performer beta failed"
fleet=$(wait_presence "$conductor_svc" "$beta_id" stale "$STALE_BUDGET")
[[ "$(presence_of "$fleet" "$alpha_id")" == "online" ]] \
    || fail "isolating one Performer changed the other Performer's presence"
[[ "$(jq -r '.data.presence.stale' <<<"$fleet")" == "1" ]] \
    || fail "expected exactly one stale Performer: $(jq -c '.data.presence' <<<"$fleet")"
[[ "$(jq -r '.data.presence.online' <<<"$fleet")" == "1" ]] \
    || fail "expected exactly one online Performer while the other is isolated"

"${docker_cmd[@]}" network connect "$network_name" "$beta_container" \
    || fail "rejoining performer beta failed"
"${compose[@]}" restart "$beta_svc" >/dev/null || fail "restarting performer beta failed"
wait_service "$beta_svc"
wait_connected "$conductor_svc" "$beta_id"
wait_presence "$conductor_svc" "$beta_id" online "$REPORT_BUDGET" >/dev/null
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
beta_sequence_after=$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE((SELECT sequence FROM health_pulses WHERE node_id = '$beta_id'), 0);")
(( beta_sequence_after > beta_sequence_before )) \
    || fail "performer beta recovered without fresh Pulse state: $beta_sequence_before -> $beta_sequence_after"

# ---------------------------------------------------------------------------
# Phase 7: restart persistence at the Conductor.
# ---------------------------------------------------------------------------
step 'certify Health Plane persistence across a real Conductor restart'
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
alpha_health_before=$(peer_health_snapshot "$tmp_dir/conductor.sqlite" "$alpha_id")
alpha_trust_before=$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$alpha_id")
beta_trust_before=$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$beta_id")
audit_before_restart=$(latest_health_audit_id "$tmp_dir/conductor.sqlite")
material_before=$(persisted_material_hashes "$conductor_svc")
identity_before=$(node_key <<<"$(exec_node "$conductor_svc" status)")
"${compose[@]}" stop "$conductor_svc" >/dev/null || fail "stopping the Conductor failed"
"${compose[@]}" start "$conductor_svc" >/dev/null || fail "starting the Conductor failed"
wait_service "$conductor_svc"
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
[[ "$alpha_health_before" == "$(peer_health_snapshot "$tmp_dir/conductor.sqlite" "$alpha_id")" ]] \
    || fail "the Conductor restart lost or mutated stored Health Plane state"
[[ "$alpha_trust_before" == "$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$alpha_id")" ]] \
    || fail "the Conductor restart changed a Performer's trust row"
[[ "$material_before" == "$(persisted_material_hashes "$conductor_svc")" ]] \
    || fail "the Conductor restart changed persisted identity or transport material"
[[ "$identity_before" == "$(node_key <<<"$(exec_node "$conductor_svc" status)")" ]] \
    || fail "the Conductor restart changed its public identity"
(( $(latest_health_audit_id "$tmp_dir/conductor.sqlite") >= audit_before_restart )) \
    || fail "the Conductor restart regressed the Health Plane audit trail"
restore_fleet_sessions
wait_presence "$conductor_svc" "$alpha_id" online "$REPORT_BUDGET" >/dev/null
wait_presence "$conductor_svc" "$beta_id" online "$REPORT_BUDGET" >/dev/null

# ---------------------------------------------------------------------------
# Phase 8: adversarial matrix over real production Noise sessions.
# ---------------------------------------------------------------------------
step 'inject the contracted adversarial matrix over production Noise'
# The matrix runs from the adversary's own identity, driven host-side over the
# Conductor's published direct listener. Using a disposable identity - rather
# than borrowing a live Performer's - is deliberate: the flood, ordering, and
# capacity cases legitimately advance *that peer's* Health Plane state, and the
# gate must still be able to prove that neither Performer's trust or health
# state moved. The adversary's own rows are purged when its trust is revoked at
# the end of the phase.
"${compose[@]}" stop "$adversary_svc" >/dev/null || fail "stopping the adversary failed"
adversary_material="$tmp_dir/adversary-material"
mkdir -p "$adversary_material"
"${compose[@]}" cp "$adversary_svc:/var/lib/omakure/." "$adversary_material" >/dev/null \
    || fail "copying adversary identity material failed"
reclaim_material "$adversary_material"
chmod 0700 "$adversary_material"
chmod 0600 "$adversary_material/identity.key" "$adversary_material/transport.key" \
    "$adversary_material/transport.cert"
adversary_material_digest=$(sha256sum \
    "$adversary_material/identity.key" "$adversary_material/transport.key" \
    "$adversary_material/transport.cert" | sha256sum)

# An untrusted peer never reaches the Health Plane at all: the shipped
# transport refuses it at admission, using the production CLI probe.
step 'certify an untrusted peer is refused before the Health Plane is reached'
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
untrusted_transport_audit_before=$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE(MAX(id), 0) FROM transport_audit;")
untrusted_health_audit_before=$(latest_health_audit_id "$tmp_dir/conductor.sqlite")
conductor_address="$(peer_ip "$conductor_svc" "$adversary_svc"):7879"
if untrusted_output=$("${compose[@]}" run --rm --no-deps -T "$adversary_svc" --json node \
    direct-probe --endpoint "$conductor_address" --peer-node-id "$conductor_id" 2>&1); then
    fail "an untrusted peer completed a production session with the Conductor"
fi
untrusted_json=$(last_json_line <<<"$untrusted_output" 2>/dev/null) \
    || fail "the untrusted probe produced no JSON envelope: $untrusted_output"
jq -e '.ok == false and .error.code == "transport_not_enrolled"' <<<"$untrusted_json" >/dev/null \
    || fail "an untrusted peer returned the wrong stable error: $untrusted_json"
untrusted_deadline=$((SECONDS + AUDIT_BUDGET))
untrusted_rejection=''
while (( SECONDS < untrusted_deadline )); do
    copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
    untrusted_rejection=$(sql "$tmp_dir/conductor.sqlite" \
        "SELECT COUNT(*) FROM transport_audit WHERE id > $untrusted_transport_audit_before AND outcome = 'rejected';")
    (( untrusted_rejection > 0 )) && break
    sleep 1
done
(( ${untrusted_rejection:-0} > 0 )) \
    || fail "the Conductor recorded no durable transport rejection for the untrusted peer"
(( $(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COUNT(*) FROM health_audit WHERE id > $untrusted_health_audit_before;") == 0 )) \
    || fail "an untrusted peer reached the Health Plane"

# One extra disposable identity, trusted at the Conductor in the *conductor*
# role. The shipped `node trust` refuses to re-register an existing peer, so
# proving `health_wrong_role` needs a peer that was trusted in that role from
# the start rather than a mutated one.
step 'mint the wrong-role identity'
role_material="$tmp_dir/role-material"
mkdir -p "$role_material"
"${docker_cmd[@]}" run --rm --user 0 -v "$role_material:/var/lib/omakure" \
    --entrypoint /bin/sh "$image" \
    -c 'chown 10001:10001 /var/lib/omakure && chmod 0700 /var/lib/omakure' >/dev/null \
    || fail "preparing the wrong-role identity directory failed"
"${docker_cmd[@]}" run --rm -v "$role_material:/var/lib/omakure" "$image" --json node init \
    >/dev/null || fail "initializing the wrong-role identity failed"
role_status=$(last_json_line <<<"$("${docker_cmd[@]}" run --rm -v "$role_material:/var/lib/omakure" \
    "$image" --json node status)")
role_id=$(node_id <<<"$role_status")
role_key=$(node_key <<<"$role_status")
role_cert=$("${docker_cmd[@]}" run --rm -v "$role_material:/var/lib/omakure" \
    --entrypoint /bin/sh "$image" -c \
    'od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d " \n"')
[[ -n "$role_cert" ]] || fail "reading the wrong-role transport certificate failed"
reclaim_material "$role_material"
chmod 0700 "$role_material"

# Seed the adversary's Signal inbox to one below the frozen capacity, so the
# matrix can exercise the bounded-overflow rejection without spending several
# real minutes inside the frozen ten-Signals-per-minute rate bound. The rows
# are ordinary stored Signals; the rejection they provoke is the production
# capacity check, unmodified.
step 'seed the adversary inbox to the frozen capacity boundary'
seeded_cursor=$((FROZEN_SIGNAL_INBOX_CAPACITY - 1))
"${compose[@]}" stop "$conductor_svc" >/dev/null || fail "stopping the Conductor failed"
trust_offline "$conductor_svc" "$adversary_id" "${node_key_of[$adversary_svc]}" \
    "${node_cert_of[$adversary_svc]}" performer "adversarial matrix subject" \
    "$FROZEN_CAPABILITY_PROFILE_PULSE" "$FROZEN_CAPABILITY_SIGNAL"
trust_offline "$conductor_svc" "$role_id" "$role_key" "$role_cert" conductor \
    "wrong-role matrix subject" "$FROZEN_CAPABILITY_PROFILE_PULSE"
seed_db="$tmp_dir/seed.sqlite"
copy_db "$conductor_svc" "$seed_db"
sql "$seed_db" "PRAGMA wal_checkpoint(TRUNCATE); VACUUM;" >/dev/null \
    || fail "checkpointing the copied registry failed"
seed_now=$(date -u +%s)
sql "$seed_db" "
INSERT INTO health_peers
  (node_id, role, cursor, last_profile_revision, last_pulse_sequence, last_pulse_at,
   version_incompatible_at, minute_window_start, minute_messages, minute_signals,
   hour_window_start, hour_profiles, first_seen, updated_at)
VALUES ('$adversary_id', 2, $seeded_cursor, 0, 0, NULL, NULL, 0, 0, 0, 0, 0, $seed_now, $seed_now);
WITH RECURSIVE seeded(n) AS (
  SELECT 1 UNION ALL SELECT n + 1 FROM seeded WHERE n < $seeded_cursor
)
INSERT INTO health_signals
  (node_id, signal_id, sequence, state, kind, occurred_at, subject, run,
   message_bytes, received_at, expires_at)
SELECT '$adversary_id', randomblob(16), n, 'applied', 'run-completed', $seed_now,
       NULL, NULL, 700, $seed_now, $seed_now + $FROZEN_SIGNAL_RETENTION_SECONDS
FROM seeded;
" || fail "seeding the adversary Signal inbox failed"
(( $(sql "$seed_db" "SELECT COUNT(*) FROM health_signals WHERE node_id = '$adversary_id';") \
    == seeded_cursor )) || fail "the Signal inbox seed did not land"
"${compose[@]}" cp "$seed_db" "$conductor_svc:/var/lib/omakure/node.sqlite" >/dev/null \
    || fail "restoring the seeded database failed"
"${docker_cmd[@]}" run --rm --user 0 \
    -v "${project}_${conductor_svc}-state:/var/lib/omakure" \
    --entrypoint /bin/sh "$image" \
    -c 'rm -f /var/lib/omakure/node.sqlite-wal /var/lib/omakure/node.sqlite-shm && chown 10001:10001 /var/lib/omakure/node.sqlite && chmod 0600 /var/lib/omakure/node.sqlite' \
    >/dev/null || fail "restoring seeded database ownership failed"
"${compose[@]}" start "$conductor_svc" >/dev/null || fail "starting the Conductor failed"
wait_service "$conductor_svc"
restore_fleet_sessions

copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
alpha_trust_before=$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$alpha_id")
beta_trust_before=$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$beta_id")
alpha_signals_before=$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE(group_concat(hex(signal_id) || ':' || sequence || ':' || state, ';'), '')
     FROM (SELECT * FROM health_signals WHERE node_id = '$alpha_id' ORDER BY sequence);")
alpha_cursor_before=$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE((SELECT cursor FROM health_peers WHERE node_id = '$alpha_id'), -1);")

OMAKURE_HP_COMPOSE_FILE="$compose_file" \
OMAKURE_HP_PROJECT="$project" \
OMAKURE_HP_CONDUCTOR_SERVICE="$conductor_svc" \
OMAKURE_HP_CONDUCTOR_ID="$conductor_id" \
OMAKURE_HP_CONDUCTOR_KEY="${node_key_of[$conductor_svc]}" \
OMAKURE_HP_CONDUCTOR_ENDPOINT="127.0.0.1:$(direct_port_of "$conductor_svc")" \
OMAKURE_HP_ADVERSARY_ID="$adversary_id" \
OMAKURE_HP_ADVERSARY_KEY="${node_key_of[$adversary_svc]}" \
OMAKURE_HP_ADVERSARY_CERT="${node_cert_of[$adversary_svc]}" \
OMAKURE_HP_ADVERSARY_STATE="$adversary_material" \
OMAKURE_HP_PERFORMER_ID="$alpha_id" \
OMAKURE_HP_SEEDED_CURSOR="$seeded_cursor" \
OMAKURE_HP_ROLE_ID="$role_id" \
OMAKURE_HP_ROLE_STATE="$role_material" \
OMAKURE_HP_TMP="$tmp_dir" \
    timeout --foreground --kill-after=30s 20m \
    cargo test --test docker_health_plane_adversary --locked -- --ignored --nocapture \
    2>&1 | tee "$tmp_dir/adversary-matrix.log" \
    || fail "the production Noise adversary matrix failed"
assert_single_test_ran "$(<"$tmp_dir/adversary-matrix.log")" 'the adversary matrix'

copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
[[ "$alpha_trust_before" == "$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$alpha_id")" ]] \
    || fail "adversarial traffic mutated performer alpha's trust, role, capability, or revocation state"
[[ "$beta_trust_before" == "$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$beta_id")" ]] \
    || fail "adversarial traffic mutated performer beta's trust, role, capability, or revocation state"
[[ "$alpha_signals_before" == "$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE(group_concat(hex(signal_id) || ':' || sequence || ':' || state, ';'), '')
     FROM (SELECT * FROM health_signals WHERE node_id = '$alpha_id' ORDER BY sequence);")" ]] \
    || fail "adversarial traffic mutated performer alpha's stored Signals"
[[ "$alpha_cursor_before" == "$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE((SELECT cursor FROM health_peers WHERE node_id = '$alpha_id'), -1);")" ]] \
    || fail "adversarial traffic moved performer alpha's Signal cursor"
[[ "$adversary_material_digest" == "$(sha256sum \
    "$adversary_material/identity.key" "$adversary_material/transport.key" \
    "$adversary_material/transport.cert" | sha256sum)" ]] \
    || fail "the adversary's identity material changed during the matrix"
assert_frozen_storage_bounds "$tmp_dir/conductor.sqlite" "the Conductor after the adversarial matrix"
assert_redacted_health_audit "$tmp_dir/conductor.sqlite"
exec_node "$conductor_svc" revoke "$role_id" --actor health-certification \
    --reason "wrong-role matrix teardown" --confirmed >/dev/null \
    || fail "revoking the wrong-role identity failed"
# The revoked adversary must be gone from the fleet projection immediately.
adversarial_fleet=$(fleet_json "$conductor_svc")
[[ "$(presence_of "$adversarial_fleet" "$adversary_id")" == "absent" ]] \
    || fail "the revoked adversary is still projected in the fleet"
[[ "$(presence_of "$adversarial_fleet" "$role_id")" == "absent" ]] \
    || fail "the revoked wrong-role peer is still projected in the fleet"

# ---------------------------------------------------------------------------
# Phase 9: corrupt stored state is quarantined and recovers.
# ---------------------------------------------------------------------------
step 'certify corrupt stored Health Plane state is quarantined and recovers'
"${compose[@]}" stop "$conductor_svc" >/dev/null || fail "stopping the Conductor failed"
corrupt_db="$tmp_dir/corrupt.sqlite"
copy_db "$conductor_svc" "$corrupt_db"
# Fold the write-ahead log into the copied database before editing it, so the
# file pushed back into the volume is the whole registry and not a prefix.
sql "$corrupt_db" "PRAGMA wal_checkpoint(TRUNCATE); VACUUM;" >/dev/null \
    || fail "checkpointing the copied registry failed"
corrupt_audit_before=$(latest_health_audit_id "$corrupt_db")
sql "$corrupt_db" \
    "UPDATE health_profiles SET runtimes = 'not-json' WHERE node_id = '$beta_id';" \
    || fail "injecting the corrupt Profile row failed"
(( $(sql "$corrupt_db" "SELECT COUNT(*) FROM health_profiles WHERE runtimes = 'not-json';") == 1 )) \
    || fail "the corrupt-state injection did not land"
"${compose[@]}" cp "$corrupt_db" "$conductor_svc:/var/lib/omakure/node.sqlite" >/dev/null \
    || fail "restoring the corrupted database failed"
"${docker_cmd[@]}" run --rm --user 0 \
    -v "${project}_${conductor_svc}-state:/var/lib/omakure" \
    --entrypoint /bin/sh "$image" \
    -c 'rm -f /var/lib/omakure/node.sqlite-wal /var/lib/omakure/node.sqlite-shm && chown 10001:10001 /var/lib/omakure/node.sqlite && chmod 0600 /var/lib/omakure/node.sqlite' \
    >/dev/null || fail "restoring corrupted database ownership failed"
"${compose[@]}" start "$conductor_svc" >/dev/null || fail "starting the Conductor failed"
wait_service "$conductor_svc"
# Reading the fleet is what forces the corrupt row through the production
# quarantine path; the row is deleted and audited with the frozen 1115 code.
fleet_json "$conductor_svc" >/dev/null || fail "the fleet projection failed after corrupt-state injection"
quarantine_row=$(wait_health_audit_code "$conductor_svc" "$corrupt_audit_before" 1115)
[[ "$quarantine_row" == *"|corrupt_row|"* ]] \
    || fail "the corrupt row was not audited as a quarantine event: $quarantine_row"
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
(( $(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COUNT(*) FROM health_profiles WHERE runtimes = 'not-json';") == 0 )) \
    || fail "the corrupt Profile row survived quarantine"
[[ "$alpha_trust_before" == "$(peer_trust_snapshot "$tmp_dir/conductor.sqlite" "$alpha_id")" ]] \
    || fail "corrupt-state quarantine mutated a Performer's trust row"
restore_fleet_sessions
wait_presence "$conductor_svc" "$beta_id" online "$REPORT_BUDGET" >/dev/null
recovery_deadline=$((SECONDS + REPORT_BUDGET))
beta_profiles=0
while (( SECONDS < recovery_deadline )); do
    copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
    beta_profiles=$(sql "$tmp_dir/conductor.sqlite" \
        "SELECT COUNT(*) FROM health_profiles WHERE node_id = '$beta_id';")
    (( beta_profiles == 1 )) && break
    sleep 3
done
(( beta_profiles == 1 )) \
    || fail "performer beta did not re-report a fresh Profile after quarantine within ${REPORT_BUDGET}s"

# ---------------------------------------------------------------------------
# Phase 10: revocation excludes a Performer immediately.
# ---------------------------------------------------------------------------
step 'certify revocation excludes a Performer from the fleet immediately'
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
(( $(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COUNT(*) FROM health_peers WHERE node_id = '$beta_id';") == 1 )) \
    || fail "performer beta has no Health Plane row to revoke"
exec_node "$conductor_svc" revoke "$beta_id" --actor health-certification \
    --reason "certified revocation" --confirmed >/dev/null || fail "revoking performer beta failed"
revoke_deadline=$((SECONDS + 30))
fleet=''
while (( SECONDS < revoke_deadline )); do
    fleet=$(fleet_json "$conductor_svc")
    [[ "$(presence_of "$fleet" "$beta_id")" == "absent" ]] && break
    sleep 1
done
[[ "$(presence_of "$fleet" "$beta_id")" == "absent" ]] \
    || fail "a revoked Performer was still projected in the fleet"
[[ "$(presence_of "$fleet" "$alpha_id")" == "online" ]] \
    || fail "revoking one Performer disturbed the other Performer"
feed=$(signals_json "$conductor_svc")
[[ "$(jq -r --arg b "$beta_id" \
    '[.data.signals[] | select(.source == "local" and .kind == "revoked" and .subject == $b)] | length' \
    <<<"$feed")" == "1" ]] || fail "revocation did not produce exactly one local revoked Signal"

# Retention removes the revoked peer's Health Plane rows on the next bounded
# maintenance pass, whose cadence is the frozen 60-second rate window.
purge_deadline=$((SECONDS + MAINTENANCE_BUDGET))
beta_rows=1
while (( SECONDS < purge_deadline )); do
    copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
    beta_rows=$(sql "$tmp_dir/conductor.sqlite" \
        "SELECT (SELECT COUNT(*) FROM health_peers WHERE node_id = '$beta_id')
              + (SELECT COUNT(*) FROM health_profiles WHERE node_id = '$beta_id')
              + (SELECT COUNT(*) FROM health_pulses WHERE node_id = '$beta_id');")
    (( beta_rows == 0 )) && break
    sleep 5
done
(( beta_rows == 0 )) \
    || fail "the revoked Performer's Health Plane rows survived the bounded retention pass"

# ---------------------------------------------------------------------------
# Phase 11: identity replacement.
# ---------------------------------------------------------------------------
step 'certify identity replacement of a Performer'
"${compose[@]}" stop "$beta_svc" >/dev/null || fail "stopping performer beta failed"
new_beta_id=''
for (( draw = 1; draw <= MAX_IDENTITY_DRAWS; draw++ )); do
    run_node "$beta_svc" reset --confirmed >/dev/null || fail "resetting performer beta failed"
    run_node "$beta_svc" init >/dev/null || fail "re-initializing performer beta failed"
    new_beta_status=$(run_node "$beta_svc" status)
    candidate=$(node_id <<<"$new_beta_status")
    # The replacement must keep sorting below the Conductor, or the shipped
    # dial-ownership rule would stop it from dialling in at all.
    if [[ "$(LC_ALL=C printf '%s\n%s\n' "$candidate" "$conductor_id" | LC_ALL=C sort | head -n 1)" \
        == "$candidate" && "$candidate" != "$conductor_id" ]]; then
        new_beta_id=$candidate
        break
    fi
done
[[ -n "$new_beta_id" ]] \
    || fail "no replacement identity sorted below the Conductor within $MAX_IDENTITY_DRAWS bounded draws"
[[ "$new_beta_id" != "$beta_id" ]] || fail "identity replacement did not replace the node identity"
new_beta_key=$(node_key <<<"$new_beta_status")
new_beta_cert=$(certificate "$beta_svc")
write_config "$beta_svc" \
    "$(static_config health-performer-beta "\"$conductor_id@$conductor_svc:7879\"")"
trust_offline "$beta_svc" "$conductor_id" "${node_key_of[$conductor_svc]}" "${node_cert_of[$conductor_svc]}" \
    conductor "certified conductor after replacement" \
    "$FROZEN_CAPABILITY_PROFILE_PULSE" "$FROZEN_CAPABILITY_SIGNAL"
exec_node "$conductor_svc" trust --node-id "$new_beta_id" --public-key "$new_beta_key" \
    --transport-certificate "$new_beta_cert" --role performer \
    --capability "$FROZEN_CAPABILITY_PROFILE_PULSE" --capability "$FROZEN_CAPABILITY_SIGNAL" \
    --actor health-certification --reason "certified replacement performer" --confirmed >/dev/null \
    || fail "trusting the replacement identity at the Conductor failed"
"${compose[@]}" start "$beta_svc" >/dev/null || fail "starting the replaced performer beta failed"
wait_service "$beta_svc"
wait_connected "$conductor_svc" "$new_beta_id"
fleet=$(wait_presence "$conductor_svc" "$new_beta_id" online "$REPORT_BUDGET")
[[ "$(presence_of "$fleet" "$beta_id")" == "absent" ]] \
    || fail "the replaced identity is still projected in the fleet"
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
[[ "$(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COALESCE((SELECT cursor FROM health_peers WHERE node_id = '$new_beta_id'), -1);")" == "0" ]] \
    || fail "the replacement identity did not start from a fresh Signal cursor"
(( $(sql "$tmp_dir/conductor.sqlite" \
    "SELECT COUNT(*) FROM health_peers WHERE node_id = '$beta_id';") == 0 )) \
    || fail "the replaced identity still holds Health Plane state"

# ---------------------------------------------------------------------------
# Phase 12: attempt exhaustion over one real, continuously connected session.
# ---------------------------------------------------------------------------
step 'certify Profile attempt exhaustion over one continuously connected session'
# A Conductor that stays connected and never acknowledges is not controllable
# black-box against the production binary, so the harness plays the Conductor:
# it is the responder for a real Noise session, accepts the probe, and then
# withholds every `health_ack`. Performer alpha is repointed at it for exactly
# this phase; the harness identity is the adversary's, which ranks above both
# Performers so the shipped dial-ownership rule lets alpha dial out to it.
# The harness runs as `hp-harness` on the dedicated network, not on the host.
# The Performer is the initiator here, and a host-side listener would need the
# container to reach the host's own address -- which any default-deny INPUT
# firewall drops, making the phase fail for reasons that have nothing to do
# with the Health Plane. On the network, Compose DNS resolves the harness and
# the phase depends on nothing but Docker.
exhaust_host=hp-harness
exhaust_port=7879
"${compose[@]}" stop "$alpha_svc" >/dev/null || fail "stopping performer alpha failed"
run_node "$alpha_svc" revoke "$conductor_id" --actor health-certification \
    --reason "attempt-exhaustion phase" --confirmed >/dev/null \
    || fail "detaching performer alpha from the real Conductor failed"
trust_offline "$alpha_svc" "$adversary_id" "${node_key_of[$adversary_svc]}" \
    "${node_cert_of[$adversary_svc]}" conductor "attempt-exhaustion listener" \
    "$FROZEN_CAPABILITY_PROFILE_PULSE" "$FROZEN_CAPABILITY_SIGNAL"
write_config "$alpha_svc" \
    "$(static_config health-performer-alpha "\"$adversary_id@$exhaust_host:$exhaust_port\"")"

# The harness reads the adversary's node material, and publishes its readiness
# marker, through a directory the runner owns and bind-mounts. The material is
# already on the host from the adversarial phase.
mkdir -p "$harness_dir"
cp -a "$adversary_material" "$harness_dir/adversary-material"
exhaust_ready="$harness_dir/exhaustion.ready"
rm -f "$exhaust_ready"
export OMAKURE_HP_ADVERSARY_ID="$adversary_id"
export OMAKURE_HP_PERFORMER_ID="$alpha_id"

# Start the harness before the Performer is repointed. The shipped dialer stops
# after its frozen three attempts, so the listener has to be accepting already;
# a container start in between would burn that budget.
"${compose[@]}" up --detach --no-deps hp-harness >/dev/null \
    || fail "starting the attempt-exhaustion harness failed"
harness_container=$("${compose[@]}" ps --quiet hp-harness)
[[ -n "$harness_container" ]] || fail "the attempt-exhaustion harness container did not start"
listener_deadline=$((SECONDS + 90))
while (( SECONDS < listener_deadline )); do
    [[ -f "$exhaust_ready" ]] && break
    sleep 1
done
if [[ ! -f "$exhaust_ready" ]]; then
    # Without this the phase reports a bare timeout and hides the harness's own
    # panic, which is the only thing that explains why it never accepted.
    "${compose[@]}" logs --no-log-prefix hp-harness >&2 || true
    fail "the attempt-exhaustion listener did not start accepting within 90s"
fi
"${compose[@]}" start "$alpha_svc" >/dev/null || fail "starting the repointed performer alpha failed"
# Bounded explicitly here rather than through `docker_cmd`, whose 120s ceiling
# is shorter than the harness's own frozen accept-plus-observation budget.
harness_status=$(timeout --foreground --kill-after=15s 6m \
    docker wait "$harness_container") \
    || fail "waiting for the attempt-exhaustion harness exceeded its bound"
harness_output=$("${compose[@]}" logs --no-log-prefix hp-harness 2>/dev/null || true)
# Surface what the harness saw on the wire even on success. A stray connection
# that the accept loop tolerated is invisible otherwise, and the difference
# between "tolerated one" and "never saw one" is the difference between a gate
# that is reliable and one that is quietly racing.
while IFS= read -r harness_line; do
    [[ -n "$harness_line" ]] && step "$harness_line"
done < <(printf '%s\n' "$harness_output" | grep '^harness: ' || true)
if [[ "$harness_status" != "0" ]]; then
    # Both sides, or the failure is only half-legible: the harness says what it
    # saw on the wire, the Performer says what it tried to dial.
    printf '%s\n' "$harness_output" >&2
    "${compose[@]}" logs --no-log-prefix "$alpha_svc" >&2 || true
    fail "attempt exhaustion over one real session was not certified"
fi
assert_single_test_ran "$harness_output" 'the attempt-exhaustion harness'

# ---------------------------------------------------------------------------
# Phase 13: final bounds, redaction, and leakage scans.
# ---------------------------------------------------------------------------
step 'assert every frozen bound and run the leakage scans'
copy_db "$conductor_svc" "$tmp_dir/conductor.sqlite"
assert_frozen_storage_bounds "$tmp_dir/conductor.sqlite" "the Conductor at the end of the gate"
assert_redacted_health_audit "$tmp_dir/conductor.sqlite"
assert_no_p1_leakage "the final fleet projection" "$(fleet_json "$conductor_svc")"
assert_no_p1_leakage "the final Signal feed" "$(signals_json "$conductor_svc")"
assert_no_log_leakage

step 'passed'
