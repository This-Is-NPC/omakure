#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
compose_file="$root_dir/compose.direct-transport.e2e.yaml"
project="omakure-direct-transport-e2e-${RANDOM}"
compose=(docker compose -f "$compose_file" -p "$project")
tmp_dir=$(mktemp -d)

cleanup() {
    local status=$?
    if (( status != 0 )); then
        "${compose[@]}" logs --no-log-prefix direct-a direct-b >&2 || true
    fi
    "${compose[@]}" down --volumes --remove-orphans >/dev/null 2>&1 || true
    rm -rf "$tmp_dir"
    exit "$status"
}
trap cleanup EXIT

run_node() {
    local service=$1
    shift
    "${compose[@]}" run --rm --no-deps -T "$service" --json node "$@"
}

"${compose[@]}" build
run_node direct-a init >/dev/null
run_node direct-b init >/dev/null
run_node direct-c init >/dev/null

a_status=$(run_node direct-a status)
b_status=$(run_node direct-b status)
a_id=$(jq -er '.data.identity.node_id' <<<"$a_status")
b_id=$(jq -er '.data.identity.node_id' <<<"$b_status")
a_key=$(jq -er '.data.identity.public_key' <<<"$a_status")
b_key=$(jq -er '.data.identity.public_key' <<<"$b_status")
a_cert=$("${compose[@]}" run --rm --no-deps -T --entrypoint sh direct-a -c 'od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d " \n"')
b_cert=$("${compose[@]}" run --rm --no-deps -T --entrypoint sh direct-b -c 'od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d " \n"')

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

"${compose[@]}" up -d direct-a direct-b >/dev/null
b_ip=$("${compose[@]}" run --rm --no-deps -T --entrypoint getent direct-a hosts direct-b | cut -d ' ' -f1)
a_ip=$("${compose[@]}" run --rm --no-deps -T --entrypoint getent direct-b hosts direct-a | cut -d ' ' -f1)

probe_a_to_b() {
    run_node direct-a direct-probe \
        --endpoint "$b_ip:7879" \
        --peer-node-id "$b_id"
}

probe_b_to_a() {
    run_node direct-b direct-probe \
        --endpoint "$a_ip:7879" \
        --peer-node-id "$a_id"
}

deadline=$((SECONDS + 30))
while :; do
    if probe_output=$(probe_a_to_b); then
        if jq -e '.ok == true and .data.accepted == true' <<<"$probe_output" >/dev/null; then
            break
        fi
    fi
    if (( SECONDS >= deadline )); then
        printf 'direct transport probe did not become ready: %s\n' "${probe_output:-<no output>}" >&2
        exit 1
    fi
    sleep 1
done

deadline=$((SECONDS + 30))
while :; do
    if probe_output=$(probe_b_to_a); then
        if jq -e '.ok == true and .data.accepted == true' <<<"$probe_output" >/dev/null; then
            break
        fi
    fi
    if (( SECONDS >= deadline )); then
        printf 'reverse direct transport probe did not become ready: %s\n' "${probe_output:-<no output>}" >&2
        exit 1
    fi
    sleep 1
done

if run_node direct-c direct-probe \
    --endpoint "$b_ip:7879" \
    --peer-node-id "$b_id" >/dev/null 2>&1; then
    printf 'untrusted direct transport probe unexpectedly succeeded\n' >&2
    exit 1
fi

"${compose[@]}" restart direct-b >/dev/null
deadline=$((SECONDS + 30))
while :; do
    if probe_output=$(probe_a_to_b); then
        if jq -e '.ok == true and .data.accepted == true' <<<"$probe_output" >/dev/null; then
            break
        fi
    fi
    if (( SECONDS >= deadline )); then
        printf 'direct transport probe did not recover after restart: %s\n' "${probe_output:-<no output>}" >&2
        exit 1
    fi
    sleep 1
done

"${compose[@]}" cp direct-b:/var/lib/omakure/node.sqlite "$tmp_dir/node.sqlite" >/dev/null
accepted=$(sqlite3 "$tmp_dir/node.sqlite" "SELECT COUNT(*) FROM transport_audit WHERE outcome = 'accepted';")
rejected=$(sqlite3 "$tmp_dir/node.sqlite" "SELECT COUNT(*) FROM transport_audit WHERE outcome = 'rejected';")
if (( accepted < 2 || rejected < 1 )); then
    printf 'unexpected transport audit counts: accepted=%s rejected=%s\n' "$accepted" "$rejected" >&2
    exit 1
fi

printf 'direct transport Docker E2E passed: accepted=%s rejected=%s\n' "$accepted" "$rejected"
