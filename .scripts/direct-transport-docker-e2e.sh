#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
compose_file="$root_dir/compose.direct-transport.e2e.yaml"
project="omakure-direct-transport-e2e-${BASHPID}"
compose=(timeout --foreground --kill-after=5s 120s docker compose -f "$compose_file" -p "$project")
docker_cmd=(timeout --foreground --kill-after=5s 30s docker)
sqlite_cmd=(timeout --foreground --kill-after=5s 20s sqlite3)
tmp_dir=$(mktemp -d)

cleanup() {
    local status=$?
    if (( status != 0 )); then
        "${compose[@]}" logs --no-log-prefix direct-a direct-b direct-c >&2 || true
    fi
    if ! "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1; then
        printf 'direct transport Docker E2E: Compose teardown failed for %s\n' "$project" >&2
        status=1
    fi
    for resource in container network volume; do
        local remaining
        if ! remaining=$("${docker_cmd[@]}" "$resource" ls -q \
            --filter "label=com.docker.compose.project=$project"); then
            printf 'direct transport Docker E2E: unable to inspect %s resources\n' "$resource" >&2
            status=1
        elif [[ -n "$remaining" ]]; then
            printf 'direct transport Docker E2E: %s resources survived teardown\n' "$resource" >&2
            status=1
        fi
    done
    rm -rf "$tmp_dir"
    exit "$status"
}
trap cleanup EXIT

# Compose interpolates service volume paths even for image-only builds.
export OMAKURE_DIRECT_A_TOKENS_FILE="$tmp_dir/direct-a.tokens.placeholder" \
    OMAKURE_DIRECT_A_CURL_FILE="$tmp_dir/direct-a.curl.placeholder" \
    OMAKURE_DIRECT_B_TOKENS_FILE="$tmp_dir/direct-b.tokens.placeholder" \
    OMAKURE_DIRECT_B_CURL_FILE="$tmp_dir/direct-b.curl.placeholder" \
    OMAKURE_DIRECT_C_TOKENS_FILE="$tmp_dir/direct-c.tokens.placeholder" \
    OMAKURE_DIRECT_C_CURL_FILE="$tmp_dir/direct-c.curl.placeholder"

run_node() {
    local service=$1
    shift
    "${compose[@]}" run --rm --no-deps -T "$service" --json node "$@"
}

generate_auth() {
    local service=$1
    local tokens_path="$tmp_dir/${service}.tokens.toml"
    local client_path="$tmp_dir/${service}.client.token"
    local curl_path="$tmp_dir/${service}.curl.conf"
    local stderr_path="$tmp_dir/${service}.token-generation.stderr"
    local token_json
    local status
    if token_json=$("${docker_cmd[@]}" run --rm --entrypoint /usr/local/bin/omakure \
        omakure-direct-transport-e2e:local --json token generate --id "direct-$service" --scope node:read \
        2>"$stderr_path"); then
        :
    else
        status=$?
        generation_failure "$service" "$status" "$stderr_path"
    fi
    printf 'version = 1\n\n' >"$tokens_path"
    jq -er '.data.tokens_file_entry' <<<"$token_json" >>"$tokens_path"
    jq -er '.data.token' <<<"$token_json" >"$client_path"
    printf 'header = "Authorization: Bearer %s"\n' "$(<"$client_path")" >"$curl_path"
    chmod 0600 "$tokens_path" "$client_path" "$curl_path"
    printf '%s|%s|%s\n' "$tokens_path" "$client_path" "$curl_path"
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

write_config() {
    local service=$1
    local peer_id=$2
    local peer_host=$3
    "${compose[@]}" run --rm --no-deps -T --user 0:0 --entrypoint sh "$service" -c \
        "cat > /etc/omakure/node.toml <<EOF
version = 1

[node]
display_name = \"\"

[api]
bind = \"127.0.0.1:7878\"

[network]
mode = \"direct\"
relays = []
static_peers = [\"${peer_id}@${peer_host}:7879\"]
direct_bind = \"0.0.0.0:7879\"
max_message_bytes = 1048576

[trust]
enrollment = \"disabled\"
allow_remote_cues = false
allow_baseline_push = false

[organization]
id = \"\"
discovery_secret_ref = \"\"
EOF"
}

status_node() {
    local service=$1
    "${compose[@]}" exec -T "$service" curl --config /run/secrets/curl.conf \
        --connect-timeout 3 --max-time 10 -fsS http://127.0.0.1:7878/v1/node/status
}

wait_connected() {
    local service=$1
    local peer_id=$2
    local deadline=$((SECONDS + 30))
    while :; do
        if status_output=$(status_node "$service" 2>/dev/null) \
            && jq -e --arg peer "$peer_id" \
                '.data.transport.connected_peer_count == 1 and .data.transport.peers[0].node_id == $peer and .data.transport.peers[0].state == "connected"' \
                <<<"$status_output" >/dev/null; then
            return
        fi
        if (( SECONDS >= deadline )); then
            printf 'static peer did not connect: %s\n' "${status_output:-<no output>}" >&2
            printf 'other node status: %s\n' "$(status_node direct-a 2>/dev/null || true)" >&2
            exit 1
        fi
        sleep 1
    done
}

"${compose[@]}" build
auth=$(generate_auth direct-a)
IFS='|' read -r direct_a_tokens direct_a_client direct_a_curl <<<"$auth"
auth=$(generate_auth direct-b)
IFS='|' read -r direct_b_tokens direct_b_client direct_b_curl <<<"$auth"
auth=$(generate_auth direct-c)
IFS='|' read -r direct_c_tokens direct_c_client direct_c_curl <<<"$auth"
export OMAKURE_DIRECT_A_TOKENS_FILE="$direct_a_tokens" OMAKURE_DIRECT_A_CURL_FILE="$direct_a_curl"
export OMAKURE_DIRECT_B_TOKENS_FILE="$direct_b_tokens" OMAKURE_DIRECT_B_CURL_FILE="$direct_b_curl"
export OMAKURE_DIRECT_C_TOKENS_FILE="$direct_c_tokens" OMAKURE_DIRECT_C_CURL_FILE="$direct_c_curl"
run_node direct-a init >/dev/null
run_node direct-b init >/dev/null
run_node direct-c init >/dev/null

a_status=$(run_node direct-a status)
b_status=$(run_node direct-b status)
c_status=$(run_node direct-c status)
a_id=$(jq -er '.data.identity.node_id' <<<"$a_status")
b_id=$(jq -er '.data.identity.node_id' <<<"$b_status")
c_id=$(jq -er '.data.identity.node_id' <<<"$c_status")
a_key=$(jq -er '.data.identity.public_key' <<<"$a_status")
b_key=$(jq -er '.data.identity.public_key' <<<"$b_status")
c_key=$(jq -er '.data.identity.public_key' <<<"$c_status")
a_cert=$("${compose[@]}" run --rm --no-deps -T --entrypoint sh direct-a -c 'od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d " \n"')
b_cert=$("${compose[@]}" run --rm --no-deps -T --entrypoint sh direct-b -c 'od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d " \n"')
c_cert=$("${compose[@]}" run --rm --no-deps -T --entrypoint sh direct-c -c 'od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d " \n"')

run_node direct-a trust \
    --node-id "$b_id" \
    --public-key "$b_key" \
    --transport-certificate "$b_cert" \
    --capability remote-run \
    --actor docker-e2e \
    --reason pretrusted \
    --confirmed >/dev/null
run_node direct-b trust \
    --node-id "$a_id" \
    --public-key "$a_key" \
    --transport-certificate "$a_cert" \
    --capability remote-run \
    --actor docker-e2e \
    --reason pretrusted \
    --confirmed >/dev/null
run_node direct-b trust \
    --node-id "$c_id" \
    --public-key "$c_key" \
    --transport-certificate "$c_cert" \
    --capability remote-run \
    --actor docker-e2e \
    --reason inbound-trust-not-static \
    --confirmed >/dev/null
run_node direct-c trust \
    --node-id "$b_id" \
    --public-key "$b_key" \
    --transport-certificate "$b_cert" \
    --capability remote-run \
    --actor docker-e2e \
    --reason probe-target \
    --confirmed >/dev/null

write_config direct-a "$b_id" direct-b
write_config direct-b "$a_id" direct-a

"${compose[@]}" up -d direct-a direct-b >/dev/null

wait_connected direct-a "$b_id"
wait_connected direct-b "$a_id"

direct_b_ip=$(
    "${compose[@]}" run --rm --no-deps -T --entrypoint sh direct-c \
        -c 'getent hosts direct-b | cut -d" " -f1'
)
if ! run_node direct-c direct-probe \
    --endpoint "${direct_b_ip}:7879" \
    --peer-node-id "$b_id" >/dev/null 2>&1; then
    printf 'active trusted inbound peer not listed in static_peers was rejected\n' >&2
    exit 1
fi

if run_node direct-c direct-probe \
    --endpoint "${direct_b_ip}:7879" \
    --peer-node-id "$a_id" >/dev/null 2>&1; then
    printf 'identity-mismatched direct transport probe unexpectedly succeeded\n' >&2
    exit 1
fi

# A configured locator may not silently connect to a different expected node.
write_config direct-a "$c_id" direct-b
"${compose[@]}" down --remove-orphans >/dev/null
"${compose[@]}" up -d direct-a direct-b >/dev/null
sleep 3
    if [[ "$(status_node direct-a | jq -r '.data.transport.expected_connected_peer_count')" != "0" ]]; then
    printf 'mismatched static peer unexpectedly connected\n' >&2
    exit 1
fi
write_config direct-a "$b_id" direct-b
"${compose[@]}" down --remove-orphans >/dev/null
"${compose[@]}" up -d direct-a direct-b >/dev/null
wait_connected direct-a "$b_id"
wait_connected direct-b "$a_id"

b_container=$("${compose[@]}" ps -q direct-b)
"${docker_cmd[@]}" network disconnect "${project}_default" "$b_container"
sleep 3
"${docker_cmd[@]}" network connect "${project}_default" "$b_container"
wait_connected direct-a "$b_id"

# Revocation must prevent a fresh session. Revocations are retained and cannot
# be silently undone by re-importing the same identity.
"${compose[@]}" stop direct-b >/dev/null
run_node direct-b revoke "$a_id" \
    --actor docker-e2e \
    --reason revoked-peer-test \
    --confirmed >/dev/null
"${compose[@]}" up -d direct-b >/dev/null
"${compose[@]}" stop direct-a >/dev/null
"${compose[@]}" up -d direct-a >/dev/null
sleep 3
if [[ "$(status_node direct-b | jq -r '.data.transport.connected_peer_count')" != "0" ]]; then
    printf 'revoked peer unexpectedly connected\n' >&2
    exit 1
fi

"${compose[@]}" cp direct-b:/var/lib/omakure/node.sqlite "$tmp_dir/node.sqlite" >/dev/null
accepted=$("${sqlite_cmd[@]}" "$tmp_dir/node.sqlite" "SELECT COUNT(*) FROM transport_audit WHERE outcome = 'accepted';")
rejected=$("${sqlite_cmd[@]}" "$tmp_dir/node.sqlite" "SELECT COUNT(*) FROM transport_audit WHERE outcome = 'rejected';")
if (( accepted < 2 || rejected < 1 )); then
    printf 'unexpected transport audit counts: accepted=%s rejected=%s\n' "$accepted" "$rejected" >&2
    exit 1
fi

printf 'direct transport Docker E2E passed: accepted=%s rejected=%s\n' "$accepted" "$rejected"
