#!/usr/bin/env bash
set -euo pipefail

# A bounded local certification, not a normal test-suite member. It boots three
# real Fedora guests and exercises the shipped service through systemd, Polkit,
# direct transport, Remote Cues, and the queue worker.
root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fixture_dir="$root_dir/.scripts/fixtures/fedora-vm-privilege"
libvirt_uri=${OMAKURE_VM_LIBVIRT_URI:-qemu:///system}
pool=${OMAKURE_VM_STORAGE_POOL:-images}
network=${OMAKURE_VM_NETWORK:-default}
project=${OMAKURE_VM_CERTIFICATION_PROJECT:-omk-priv-${BASHPID}}
base_volume=${OMAKURE_VM_BASE_VOLUME:-omakure-fedora-44-1.5-base.qcow2}
image_url=${OMAKURE_VM_IMAGE_URL:-https://download.fedoraproject.org/pub/fedora/linux/releases/test/44_Beta/Cloud/x86_64/images/Fedora-Cloud-Base-Generic-44_Beta-1.5.x86_64.qcow2}
image_sha256=${OMAKURE_VM_IMAGE_SHA256:-28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f}
induced_failure=${OMAKURE_VM_CERTIFICATION_INDUCE_FAILURE:-}
inspection_failure=${OMAKURE_VM_CERTIFICATION_INDUCE_INSPECTION_FAILURE:-}
tmp_dir=$(mktemp -d "${TMPDIR:-/tmp}/omakure-vm-certification.XXXXXX")
cache_dir=${XDG_CACHE_HOME:-$HOME/.cache}/omakure-vm-certification
known_hosts="$tmp_dir/known_hosts"
key_path="$tmp_dir/id_ed25519"
artifact="$tmp_dir/omakure.tar.gz"
cert_binary="$root_dir/target/x86_64-unknown-linux-musl/release/omakure"
roles=(conductor root delegated)
declare -a domains=()
declare -a volumes=()
declare -A ips=()
declare -A macs=()
declare -A node_ids=()
declare -A node_keys=()
declare -A node_certs=()

virsh_cmd=(timeout --foreground --kill-after=5s 60s virsh -c "$libvirt_uri")
virsh_quiet=(timeout --foreground --kill-after=5s 60s virsh -q -c "$libvirt_uri")
ssh_options=(
    -i "$key_path"
    -o BatchMode=yes
    -o ConnectTimeout=5
    -o ServerAliveInterval=5
    -o ServerAliveCountMax=3
    -o StrictHostKeyChecking=no
    -o UserKnownHostsFile="$known_hosts"
    -o LogLevel=ERROR
)

fail() {
    printf 'Fedora VM privilege certification: %s\n' "$*" >&2
    exit 1
}

require_cmd() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required"
}

domain_for() {
    printf '%s-%s\n' "$project" "$1"
}

disk_for() {
    printf '%s-%s.qcow2\n' "$project" "$1"
}

seed_for() {
    printf '%s-%s-seed.iso\n' "$project" "$1"
}

ssh_vm() {
    local role=$1
    shift
    timeout --foreground --kill-after=5s 300s \
        ssh "${ssh_options[@]}" "harness@${ips[$role]}" "$@"
}

employee_ssh() {
    local role=$1
    shift
    timeout --foreground --kill-after=5s 60s \
        ssh "${ssh_options[@]}" "employee@${ips[$role]}" "$@"
}

scp_to_vm() {
    local role=$1
    shift
    timeout --foreground --kill-after=5s 300s \
        scp "${ssh_options[@]}" "$@" "harness@${ips[$role]}:/home/harness/vm-cert/"
}

ipv4_from_output() {
    local output=$1 field
    for field in $output; do
        if [[ "$field" =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}/[0-9]+$ ]]; then
            printf '%s\n' "${field%/*}"
            return
        fi
    done
    return 1
}

domain_mac() {
    local domain=$1 output field
    output=$("${virsh_quiet[@]}" domiflist "$domain")
    for field in $output; do
        if [[ "$field" =~ ^([0-9a-f]{2}:){5}[0-9a-f]{2}$ ]]; then
            printf '%s\n' "$field"
            return
        fi
    done
    return 1
}

virsh_inspect() {
    local operation=$1
    shift
    if [[ "$inspection_failure" == "$operation" || "$inspection_failure" == both ]]; then
        printf 'induced inspection failure: virsh %s\n' "$operation" >&2
        return 1
    fi
    "${virsh_quiet[@]}" "$operation" "$@"
}

collect_diagnostics() {
    local role domain mac lease_output candidate
    printf 'Fedora VM privilege certification: diagnostics for %s\n' "$project" >&2
    for role in "${roles[@]}"; do
        domain=$(domain_for "$role")
        if "${virsh_cmd[@]}" dominfo "$domain" >/dev/null 2>&1; then
            printf '\n[%s: domain]\n' "$role" >&2
            "${virsh_cmd[@]}" dominfo "$domain" >&2 || true
            "${virsh_cmd[@]}" domstate "$domain" --reason >&2 || true
            printf '\n[%s: interfaces]\n' "$role" >&2
            "${virsh_cmd[@]}" domiflist "$domain" >&2 || true
            "${virsh_cmd[@]}" domifaddr "$domain" --full --source lease >&2 || true
            mac=${macs[$role]:-}
            if [[ -z "$mac" ]]; then
                mac=$(domain_mac "$domain" 2>/dev/null || true)
            fi
            if [[ -n "$mac" ]]; then
                printf '\n[%s: DHCP lease for %s]\n' "$role" "$mac" >&2
                lease_output=$("${virsh_cmd[@]}" net-dhcp-leases "$network" --mac "$mac" 2>&1 || true)
                printf '%s\n' "$lease_output" >&2
                if [[ -z "${ips[$role]:-}" ]] && candidate=$(ipv4_from_output "$lease_output"); then
                    ips[$role]=$candidate
                fi
            fi
        fi
        if [[ -n "${ips[$role]:-}" ]]; then
            printf '\n[%s: cloud-init status]\n' "$role" >&2
            ssh_vm "$role" sudo cloud-init status --long >&2 || true
            ssh_vm "$role" sudo journalctl -u cloud-init.service -u cloud-final.service -n 120 --no-pager >&2 || true
            printf '\n[%s: omakure-node journal]\n' "$role" >&2
            ssh_vm "$role" sudo journalctl -u omakure-node.service -n 120 --no-pager >&2 || true
            if [[ "$role" != root ]]; then
                printf '\n[%s: live node status]\n' "$role" >&2
                node_api_status "$role" >&2 || true
            fi
            if [[ "$role" == root ]]; then
                printf '\n[root: comparison API and worker journals]\n' >&2
                ssh_vm root sudo journalctl -u omakure-root-api.service \
                    -u omakure-root-worker.service -n 120 --no-pager >&2 || true
            fi
            printf '\n[%s: resource and crash diagnostics]\n' "$role" >&2
            ssh_vm "$role" free -h >&2 || true
            ssh_vm "$role" \
                "sudo journalctl -k --no-pager --grep='Out of memory|Killed process'" \
                >&2 || true
            if ssh_vm "$role" sudo coredumpctl -q -1 --no-pager \
                info /usr/local/bin/omakure >&2; then
                printf '\n[%s: symbolized coredump backtrace]\n' "$role" >&2
                ssh_vm "$role" \
                    "sudo coredumpctl -q -1 --debugger-arguments=\"-batch -ex 'set pagination off' -ex 'thread apply all bt'\" debug /usr/local/bin/omakure" \
                    >&2 || true
            fi
            printf '\n[%s: certified operation journal]\n' "$role" >&2
            ssh_vm "$role" sudo journalctl -u omakure-certified-root-operation.service -n 60 --no-pager >&2 || true
        fi
    done
}

cleanup() {
    local exit_status=$? role domain volume
    trap - EXIT INT TERM
    if (( exit_status != 0 )); then
        collect_diagnostics || true
    fi

    for role in "${roles[@]}"; do
        domain=$(domain_for "$role")
        if "${virsh_cmd[@]}" dominfo "$domain" >/dev/null 2>&1; then
            "${virsh_cmd[@]}" destroy "$domain" >/dev/null 2>&1 || true
            if ! "${virsh_cmd[@]}" undefine "$domain" --nvram >/dev/null 2>&1; then
                "${virsh_cmd[@]}" undefine "$domain" >/dev/null 2>&1 || exit_status=1
            fi
        fi
    done
    for volume in "${volumes[@]}"; do
        "${virsh_cmd[@]}" vol-delete --pool "$pool" "$volume" >/dev/null 2>&1 || exit_status=1
    done

    if ! verify_cleanup; then
        exit_status=1
    fi
    rm -rf "$tmp_dir"
    exit "$exit_status"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

verify_cleanup() {
    local domain_output volume_output domain volume remaining='' inspection_status=0
    if domain_output=$(virsh_inspect list --all --name 2>&1); then
        while IFS= read -r domain; do
            [[ "$domain" == "$project"-* ]] && remaining+=" domain:$domain"
        done <<<"$domain_output"
    else
        printf 'Fedora VM privilege certification: unable to inspect remaining domains: %s\n' \
            "$domain_output" >&2
        inspection_status=1
    fi
    if volume_output=$(virsh_inspect vol-list --pool "$pool" 2>&1); then
        while read -r volume _; do
            [[ -n "$volume" ]] || continue
            [[ "$volume" == "$project"-* ]] && remaining+=" volume:$volume"
        done <<<"$volume_output"
    else
        printf 'Fedora VM privilege certification: unable to inspect remaining volumes: %s\n' \
            "$volume_output" >&2
        inspection_status=1
    fi
    if [[ -n "$remaining" ]]; then
        printf 'Fedora VM privilege certification: resources survived teardown:%s\n' "$remaining" >&2
        inspection_status=1
    fi
    return "$inspection_status"
}

case "$project" in
    ''|*[!a-zA-Z0-9_-]*) fail "project must contain only letters, digits, underscores, and hyphens" ;;
esac
(( ${#project} <= 40 )) || fail "project must be at most 40 characters"
case "$base_volume" in
    ''|*/*|*'..'*) fail "base volume must be a plain libvirt volume name" ;;
esac
[[ "$image_sha256" =~ ^[0-9a-f]{64}$ ]] || fail "image checksum must be lowercase SHA-256"
case "$inspection_failure" in
    ''|list|vol-list|both) ;;
    *) fail "inspection failure must be list, vol-list, or both" ;;
esac

for command in cargo curl jq scp sha256sum ssh ssh-keygen stat tar timeout virsh virt-install xorriso; do
    require_cmd "$command"
done
[[ -r /dev/kvm && -w /dev/kvm ]] || fail "/dev/kvm must be readable and writable"
[[ -d "$fixture_dir" ]] || fail "fixture directory is missing"
pool_info=$("${virsh_cmd[@]}" pool-info "$pool")
[[ "$pool_info" == *$'State:          running'* ]] || fail "libvirt storage pool $pool is not running"
[[ "$("${virsh_quiet[@]}" net-info "$network" | while read -r key value; do [[ "$key" == Active: ]] && printf '%s' "$value"; done)" == yes ]] \
    || fail "libvirt network $network is not active"

ensure_base_volume() {
    local path actual cache_image part size
    if "${virsh_cmd[@]}" vol-info --pool "$pool" "$base_volume" >/dev/null 2>&1; then
        path=$("${virsh_quiet[@]}" vol-path --pool "$pool" "$base_volume")
        if [[ -r "$path" ]]; then
            actual=$(sha256sum "$path")
            actual=${actual%% *}
        else
            cache_image="$tmp_dir/base-volume.qcow2"
            "${virsh_cmd[@]}" vol-download --pool "$pool" "$base_volume" "$cache_image" >/dev/null
            actual=$(sha256sum "$cache_image")
            actual=${actual%% *}
        fi
        [[ "$actual" == "$image_sha256" ]] \
            || fail "existing base volume $base_volume has checksum $actual, expected $image_sha256"
        return
    fi

    mkdir -p "$cache_dir"
    cache_image="$cache_dir/${image_sha256}.qcow2"
    if [[ ! -f "$cache_image" ]]; then
        part="$cache_image.part-${BASHPID}"
        printf 'Fedora VM privilege certification: download pinned Fedora image\n'
        curl --fail --location --retry 3 --connect-timeout 15 --max-time 1800 \
            "$image_url" --output "$part"
        actual=$(sha256sum "$part")
        actual=${actual%% *}
        [[ "$actual" == "$image_sha256" ]] \
            || fail "downloaded Fedora image has checksum $actual, expected $image_sha256"
        mv "$part" "$cache_image"
    fi
    actual=$(sha256sum "$cache_image")
    actual=${actual%% *}
    [[ "$actual" == "$image_sha256" ]] || fail "cached Fedora image checksum mismatch"
    size=$(stat -c '%s' "$cache_image")
    if ! "${virsh_cmd[@]}" vol-create-as --pool "$pool" --name "$base_volume" \
        --capacity "$size" --format qcow2 >/dev/null; then
        fail "failed to create base volume $base_volume"
    fi
    if ! "${virsh_cmd[@]}" vol-upload --pool "$pool" "$base_volume" "$cache_image" >/dev/null; then
        "${virsh_cmd[@]}" vol-delete --pool "$pool" "$base_volume" >/dev/null 2>&1 || true
        fail "failed to upload base volume $base_volume"
    fi
    "${virsh_cmd[@]}" pool-refresh "$pool" >/dev/null
}

build_artifact() {
    local version
    printf 'Fedora VM privilege certification: build current release binary\n'
    CARGO_PROFILE_RELEASE_DEBUG=1 cargo build --manifest-path "$root_dir/Cargo.toml" --release --locked \
        --target x86_64-unknown-linux-musl
    version=$(cargo metadata --manifest-path "$root_dir/Cargo.toml" --locked --no-deps --format-version 1 \
        | jq -er '.packages[] | select(.name == "omakure") | .version')
    tar -czf "$artifact" -C "$(dirname "$cert_binary")" omakure
    printf '%s\n' "$version" >"$tmp_dir/version"
}

generate_auth() {
    local role=$1 token_json tokens_path client_path
    tokens_path="$tmp_dir/$role.tokens.toml"
    client_path="$tmp_dir/$role.client.token"
    if [[ "$role" == root ]]; then
        token_json=$("$cert_binary" --json token generate \
            --id "vm-cert-$role" --scope admin:status --scope runs:read --scope runs:write)
    else
        token_json=$("$cert_binary" --json token generate \
            --id "vm-cert-$role" --scope node:read --scope node:write)
    fi
    printf 'version = 1\n\n' >"$tokens_path"
    jq -er '.data.tokens_file_entry' <<<"$token_json" >>"$tokens_path"
    jq -er '.data.token' <<<"$token_json" >"$client_path"
    chmod 0600 "$tokens_path" "$client_path"
}

create_seed() {
    local role=$1 domain=$2 public_key=$3 seed_path seed_volume seed_size
    local seed_dir="$tmp_dir/seed-$role"
    seed_path="$tmp_dir/$domain-seed.iso"
    seed_volume=$(seed_for "$role")
    mkdir -p "$seed_dir"
    cat >"$seed_dir/meta-data" <<EOF
instance-id: $domain
local-hostname: $domain
EOF
    cat >"$seed_dir/user-data" <<EOF
#cloud-config
ssh_pwauth: false
disable_root: true
users:
  - name: harness
    gecos: VM certification administrator
    groups: [wheel]
    sudo: ALL=(ALL) NOPASSWD:ALL
    shell: /bin/bash
    lock_passwd: true
    ssh_authorized_keys:
      - $public_key
  - name: employee
    gecos: Unprivileged employee
    shell: /bin/bash
    lock_passwd: true
    ssh_authorized_keys:
      - $public_key
package_update: true
packages:
  - curl
  - firewalld
  - gdb
  - git
  - jq
  - polkit
runcmd:
  - [chmod, '0700', /home/harness]
  - [systemctl, enable, --now, sshd.service]
  - [systemctl, enable, --now, firewalld.service]
  - [firewall-cmd, --permanent, --add-port=7879/tcp]
  - [firewall-cmd, --permanent, --add-port=7878/tcp]
  - [firewall-cmd, --reload]
EOF
    xorriso -as mkisofs -quiet -output "$seed_path" -volid cidata \
        -joliet -rock -graft-points \
        "user-data=$seed_dir/user-data" "meta-data=$seed_dir/meta-data"
    seed_size=$(stat -c '%s' "$seed_path")
    "${virsh_cmd[@]}" vol-create-as --pool "$pool" --name "$seed_volume" \
        --capacity "$seed_size" --format raw >/dev/null
    volumes+=("$seed_volume")
    "${virsh_cmd[@]}" vol-upload --pool "$pool" "$seed_volume" "$seed_path" >/dev/null
}

create_vm() {
    local role=$1 public_key=$2 domain disk seed
    domain=$(domain_for "$role")
    disk=$(disk_for "$role")
    seed=$(seed_for "$role")
    create_seed "$role" "$domain" "$public_key"
    "${virsh_cmd[@]}" vol-create-as --pool "$pool" --name "$disk" \
        --capacity 20G --format qcow2 --backing-vol "$base_volume" \
        --backing-vol-format qcow2 >/dev/null
    volumes+=("$disk")
    timeout --foreground --kill-after=10s 180s virt-install \
        --connect "$libvirt_uri" \
        --name "$domain" \
        --memory 2048 \
        --vcpus 2 \
        --import \
        --osinfo detect=on,require=off \
        --disk "vol=$pool/$disk,bus=virtio" \
        --disk "vol=$pool/$seed,device=cdrom" \
        --network "network=$network,model=virtio" \
        --graphics none \
        --noautoconsole \
        --wait 0 >/dev/null
    domains+=("$domain")
}

wait_for_ip() {
    local role=$1 domain mac candidate domif_output lease_output
    local deadline=$((SECONDS + 180)) next_report=$SECONDS
    domain=$(domain_for "$role")
    if ! mac=$(domain_mac "$domain"); then
        fail "$role has no discoverable libvirt interface MAC"
    fi
    macs[$role]=$mac
    while (( SECONDS < deadline )); do
        domif_output=$("${virsh_cmd[@]}" domifaddr "$domain" --full --source lease 2>&1 || true)
        if candidate=$(ipv4_from_output "$domif_output"); then
            ips[$role]=$candidate
            return
        fi
        lease_output=$("${virsh_cmd[@]}" net-dhcp-leases "$network" --mac "$mac" 2>&1 || true)
        if candidate=$(ipv4_from_output "$lease_output"); then
            ips[$role]=$candidate
            return
        fi
        if (( SECONDS >= next_report )); then
            printf 'Fedora VM privilege certification: waiting for %s DHCP lease (MAC %s)\n' \
                "$role" "$mac"
            next_report=$((SECONDS + 30))
        fi
        sleep 2
    done
    printf 'Fedora VM privilege certification: last %s domifaddr output:\n%s\n' \
        "$role" "$domif_output" >&2
    printf 'Fedora VM privilege certification: last %s DHCP lease output:\n%s\n' \
        "$role" "$lease_output" >&2
    fail "$role did not obtain a DHCP address for MAC $mac"
}

wait_for_ssh() {
    local role=$1 deadline=$((SECONDS + 180)) ssh_output='' cloud_output='' cloud_long=''
    while (( SECONDS < deadline )); do
        if ssh_output=$(ssh_vm "$role" true 2>&1); then
            if cloud_output=$(ssh_vm "$role" sudo cloud-init status --wait 2>&1); then
                :
            else
                cloud_long=$(ssh_vm "$role" sudo cloud-init status --long 2>&1 || true)
                if [[ "$cloud_long" != *$'status: done'* || "$cloud_long" != *$'errors: []'* ]]; then
                    fail "$role cloud-init failed: $cloud_output; details: $cloud_long"
                fi
                printf 'Fedora VM privilege certification: %s cloud-init completed with recoverable warnings; verify required state\n' \
                    "$role"
            fi
            if ! ssh_vm "$role" \
                'command -v curl >/dev/null && command -v git >/dev/null && command -v jq >/dev/null && id harness >/dev/null && id employee >/dev/null && sudo systemctl is-active --quiet sshd.service firewalld.service && sudo firewall-cmd --quiet --query-port=7879/tcp && sudo firewall-cmd --quiet --query-port=7878/tcp'; then
                fail "$role cloud-init completed without the required users, tools, services, or firewall rule"
            fi
            return
        fi
        sleep 2
    done
    fail "$role did not reach SSH readiness at ${ips[$role]}: ${ssh_output:-no SSH response}"
}

provision_guest() {
    local role=$1 version
    version=$(<"$tmp_dir/version")
    ssh_vm "$role" install -d -m 0700 /home/harness/vm-cert
    scp_to_vm "$role" "$artifact" "$root_dir/install.sh" \
        "$tmp_dir/$role.tokens.toml" "$tmp_dir/$role.client.token"
    if [[ "$role" == conductor ]]; then
        scp_to_vm "$role" "$tmp_dir/root.client.token"
    fi
    timeout --foreground --kill-after=5s 300s scp "${ssh_options[@]}" -r \
        "$fixture_dir" "harness@${ips[$role]}:/home/harness/vm-cert/fixture"
    ssh_vm "$role" mv "/home/harness/vm-cert/$role.tokens.toml" /home/harness/vm-cert/tokens.toml
    ssh_vm "$role" mv "/home/harness/vm-cert/$role.client.token" /home/harness/vm-cert/client.token
    ssh_vm "$role" bash /home/harness/vm-cert/fixture/guest-provision.sh "$role" "$version"
}

workspace_cli() {
    local role=$1
    shift
    if [[ "$role" == root ]]; then
        ssh_vm "$role" sudo env HOME=/root /usr/local/bin/omakure \
            --scripts-dir /var/lib/omakure-workspace --json "$@"
    else
        ssh_vm "$role" sudo -u omakure env HOME=/var/lib/omakure-workspace \
            /usr/local/bin/omakure --scripts-dir /var/lib/omakure-workspace --json "$@"
    fi
}

node_cli() {
    local role=$1
    shift
    workspace_cli "$role" node "$@"
}

read_identity() {
    local role=$1 init status
    if ! init=$(node_cli "$role" init); then
        fail "$role identity initialization failed: $init"
    fi
    if ! status=$(node_cli "$role" status); then
        fail "$role status read failed after initialization: $status"
    fi
    node_ids[$role]=$(jq -er '.data.identity.node_id' <<<"$status")
    node_keys[$role]=$(jq -er '.data.identity.public_key' <<<"$status")
    node_certs[$role]=$(ssh_vm "$role" "sudo od -An -tx1 -v /var/lib/omakure/transport.cert | tr -d ' \\n'")
    [[ "${node_certs[$role]}" =~ ^[0-9a-f]+$ ]] || fail "$role transport certificate was not hexadecimal"
}

write_node_config() {
    local role=$1 peers allow batteries config="$tmp_dir/$role.node.toml"
    case "$role" in
        conductor)
            peers="\"${node_ids[delegated]}@${ips[delegated]}:7879\""
            allow=false
            batteries=''
            ;;
        delegated)
            peers="\"${node_ids[conductor]}@${ips[conductor]}:7879\""
            allow=true
            batteries='"certified-privilege"'
            ;;
        *) fail "node config is not defined for role $role" ;;
    esac
    cat >"$config" <<EOF
version = 1

[node]
display_name = "vm-cert-$role"

[api]
bind = "127.0.0.1:7878"

[network]
mode = "direct"
relays = []
direct_bind = "0.0.0.0:7879"
static_peers = [$peers]
max_message_bytes = 1048576

[trust]
enrollment = "manual"
allow_remote_cues = $allow
remote_cue_scripts = []
remote_cue_batteries = [$batteries]
allow_baseline_push = false
baseline_publishers = []
authorities = []
bootstrap_token_hash = ""
bootstrap_nonce_hash = ""

[discovery]
enabled = false
port = 38383
multicast_addr = "239.255.42.99"
broadcast = true

[organization]
id = "vm-certification"
discovery_secret_ref = ""
EOF
    scp_to_vm "$role" "$config"
    ssh_vm "$role" sudo install -o root -g omakure -m 0640 \
        "/home/harness/vm-cert/$role.node.toml" /etc/omakure/node.toml
}

trust_peer() {
    local owner=$1 peer=$2 role=$3
    node_cli "$owner" trust \
        --node-id "${node_ids[$peer]}" \
        --public-key "${node_keys[$peer]}" \
        --transport-certificate "${node_certs[$peer]}" \
        --role "$role" \
        --capability inventory-health \
        --capability notifications \
        --capability remote-run \
        --actor vm-certification \
        --reason vm-certification \
        --confirmed >/dev/null
}

node_api_status() {
    local role=$1
    ssh_vm "$role" \
        'token=$(cat /home/harness/.omakure-client-token); curl --fail --silent --show-error --connect-timeout 3 --max-time 10 -H "Authorization: Bearer $token" http://127.0.0.1:7878/v1/node/status'
}

wait_for_connections() {
    local role=$1 expected=$2 deadline=$((SECONDS + 120)) status
    while (( SECONDS < deadline )); do
        if status=$(node_api_status "$role" 2>/dev/null) \
            && jq -e --argjson expected "$expected" \
                '.data.transport.expected_peer_count == $expected and .data.transport.expected_connected_peer_count == $expected' \
                <<<"$status" >/dev/null; then
            return
        fi
        sleep 2
    done
    fail "$role did not connect to $expected expected peer(s); last status=${status:-<none>}"
}

dispatch_cue() {
    local peer=$1 script=$2 wait_seconds=$3 body
    body=$(jq -cn \
        --arg peer "${node_ids[$peer]}" \
        --arg script "$script" \
        --arg reason vm-certification \
        --argjson wait "$wait_seconds" \
        '{peer_node_id:$peer, script:$script, reason:$reason, wait_seconds:$wait}')
    printf '%s' "$body" | ssh_vm conductor \
        'token=$(cat /home/harness/.omakure-client-token); curl --silent --show-error --connect-timeout 3 --max-time 180 -H "Content-Type: application/json" -H "Authorization: Bearer $token" --data-binary @- http://127.0.0.1:7878/v1/node/cues'
}

root_api_request() {
    local method=$1 path=$2 body=${3:-}
    if [[ -n "$body" ]]; then
        printf '%s' "$body" | ssh_vm conductor \
            "token=\$(cat /home/harness/.root-client-token); curl --silent --show-error --connect-timeout 3 --max-time 30 --request '$method' -H 'Content-Type: application/json' -H \"Authorization: Bearer \$token\" --data-binary @- 'http://${ips[root]}:7878$path'"
    else
        ssh_vm conductor \
            "token=\$(cat /home/harness/.root-client-token); curl --silent --show-error --connect-timeout 3 --max-time 30 --request '$method' -H \"Authorization: Bearer \$token\" 'http://${ips[root]}:7878$path'"
    fi
}

wait_for_root_api() {
    local deadline=$((SECONDS + 45)) response=''
    while (( SECONDS < deadline )); do
        if response=$(root_api_request GET /v1/ready 2>/dev/null) \
            && jq -e '.ok == true' <<<"$response" >/dev/null; then
            return
        fi
        sleep 1
    done
    fail "root comparison API did not become ready: ${response:-no response}"
}

enqueue_root_run() {
    local body
    body=$(jq -cn '{script:"certified-root-operation.sh", actor:"corporate-conductor", reason:"root comparison certification"}')
    root_api_request POST /v1/runs "$body"
}

wait_for_root_run() {
    local run_id=$1 deadline=$((SECONDS + 45)) response=''
    while (( SECONDS < deadline )); do
        if response=$(root_api_request GET "/v1/runs/$run_id" 2>/dev/null) \
            && jq -e '.ok == true and .data.state == "completed" and .data.success == true' \
                <<<"$response" >/dev/null; then
            printf '%s\n' "$response"
            return
        fi
        sleep 1
    done
    fail "root comparison run $run_id did not complete: ${response:-no response}"
}

effect_count() {
    local role=$1 count
    count=$(ssh_vm "$role" "sudo sh -c 'if test -f /var/lib/omakure-certified-root/effects.log; then wc -l </var/lib/omakure-certified-root/effects.log; else printf 0; fi'")
    printf '%s\n' "${count//[[:space:]]/}"
}

wait_for_effect() {
    local role=$1 expected=$2 deadline=$((SECONDS + 45)) count
    while (( SECONDS < deadline )); do
        count=$(effect_count "$role")
        [[ "$count" == "$expected" ]] && return
        sleep 1
    done
    fail "$role effect count did not reach $expected; observed $count"
}

assert_effect_audit() {
    local role=$1 expected=$2 history effects signals expected_run_id=$3
    [[ "$(effect_count "$role")" == "$expected" ]] || fail "$role executed an unexpected number of root operations"
    effects=$(ssh_vm "$role" sudo cat /var/lib/omakure-certified-root/effects.log)
    while IFS= read -r line; do
        [[ "$line" == uid=0\ operation=fixed\ timestamp=* ]] || fail "$role recorded a non-root or non-fixed effect: $line"
    done <<<"$effects"
    history=$(workspace_cli "$role" history list --script certified-root-operation.sh --state-set all --limit 10)
    jq -e '.ok == true and (.data | any(.trigger == "Cue" and .state == "completed" and .success == true))' \
        <<<"$history" >/dev/null || fail "$role history lacks a successful Cue-origin run"
    signals=$(node_cli conductor signals)
    jq -e --arg run "$expected_run_id" \
        '.ok == true and (.data.signals | any(.kind == "run-completed" and .run.run_id == $run))' \
        <<<"$signals" >/dev/null || fail "Conductor signal feed lacks $role outcome $expected_run_id"
}

assert_root_effect_audit() {
    local run_id=$1 response history effects
    [[ "$(effect_count root)" == 1 ]] || fail "root comparison executed an unexpected number of operations"
    effects=$(ssh_vm root sudo cat /var/lib/omakure-certified-root/effects.log)
    [[ "$effects" == uid=0\ operation=fixed\ timestamp=* ]] \
        || fail "root comparison recorded a non-root or non-fixed effect: $effects"
    response=$(root_api_request GET "/v1/runs/$run_id")
    jq -e '.ok == true and .data.state == "completed" and .data.success == true and .data.actor == "corporate-conductor"' \
        <<<"$response" >/dev/null || fail "root comparison API lacks the successful Conductor run: $response"
    history=$(workspace_cli root history list --script certified-root-operation.sh --state-set all --limit 10)
    jq -e --arg run "$run_id" \
        '.ok == true and (.data | any(.run_id == $run and .state == "completed" and .success == true))' \
        <<<"$history" >/dev/null || fail "root comparison history lacks run $run_id"
}

assert_service_mode() {
    local role=$1 expected_user=$2 expected_nnp=$3 show
    if [[ "$role" == root ]]; then
        show=$(ssh_vm root sudo systemctl show omakure-root-api.service \
            omakure-root-worker.service --property=User --property=Group \
            --property=NoNewPrivileges --property=ActiveState)
        [[ "$show" == *"ActiveState=active"* ]] || fail "root comparison services are not active: $show"
    else
        show=$(ssh_vm "$role" sudo systemctl show omakure-node.service \
            --property=User --property=Group --property=NoNewPrivileges)
    fi
    [[ "$show" == *"User=$expected_user"* && "$show" == *"NoNewPrivileges=$expected_nnp"* ]] \
        || fail "$role service mode mismatch: $show"
}

assert_employee_boundary() {
    local groups code
    groups=$(employee_ssh delegated id -nG)
    for group in $groups; do
        [[ "$group" != wheel && "$group" != omakure ]] \
            || fail "employee unexpectedly belongs to privileged group $group"
    done
    if employee_ssh delegated sudo -n true >/dev/null 2>&1; then
        fail "employee unexpectedly acquired sudo"
    fi
    employee_ssh delegated \
        'test ! -r /etc/omakure/tokens.toml && test ! -r /var/lib/omakure/identity.key && test ! -r /home/harness/.omakure-client-token'
    employee_ssh delegated \
        'test ! -w /var/lib/omakure-workspace && test ! -w /var/lib/omakure-workspace/certified-root-operation.sh && test ! -w /etc/systemd/system/omakure-certified-root-operation.service && test ! -w /etc/polkit-1/rules.d/50-omakure-certified-operation.rules'
    if employee_ssh delegated systemctl --no-ask-password start \
        omakure-certified-root-operation.service >/dev/null 2>&1; then
        fail "employee directly started the certified root unit"
    fi
    code=$(employee_ssh delegated \
        "curl --silent --output /dev/null --write-out '%{http_code}' http://127.0.0.1:7878/v1/node/status")
    [[ "$code" == 401 ]] || fail "employee reached the management API without a token; HTTP $code"
    if ssh_vm delegated sudo -u omakure systemctl --no-ask-password restart sshd.service \
        >/dev/null 2>&1; then
        fail "omakure service user managed an arbitrary systemd unit"
    fi
    [[ "$(ssh_vm delegated "sudo stat -c '%U:%G %a' /etc/polkit-1/rules.d/50-omakure-certified-operation.rules")" == "root:root 644" ]] \
        || fail "delegated Polkit rule ownership or mode changed"
}

printf 'Fedora VM privilege certification: preflight and base image\n'
ensure_base_volume
build_artifact
ssh-keygen -q -t ed25519 -N '' -f "$key_path"
public_key=$(<"$key_path.pub")
for role in "${roles[@]}"; do
    generate_auth "$role"
    printf 'Fedora VM privilege certification: create %s VM\n' "$role"
    create_vm "$role" "$public_key"
    if [[ "$induced_failure" == after-first-vm ]]; then
        fail "induced failure after first VM"
    fi
done

for role in "${roles[@]}"; do
    wait_for_ip "$role"
    wait_for_ssh "$role"
    printf 'Fedora VM privilege certification: provision %s at %s\n' "$role" "${ips[$role]}"
    provision_guest "$role"
    if [[ "$role" != root ]]; then
        read_identity "$role"
    fi
    if [[ "$induced_failure" == "after-$role-provision" ]]; then
        fail "induced failure after $role provision"
    fi
done

for role in conductor delegated; do
    write_node_config "$role"
done
trust_peer delegated conductor conductor
trust_peer conductor delegated performer

for role in delegated conductor; do
    ssh_vm "$role" sudo systemctl enable --now omakure-node.service >/dev/null
done
ssh_vm root sudo systemctl start omakure-root-api.service omakure-root-worker.service
wait_for_root_api
wait_for_connections conductor 1
wait_for_connections delegated 1

printf 'Fedora VM privilege certification: root API/worker comparison gate (intentionally broad)\n'
assert_service_mode root root no
root_response=$(enqueue_root_run)
jq -e '.ok == true and (.data.run_id | type == "string")' \
    <<<"$root_response" >/dev/null || fail "root comparison enqueue was not accepted: $root_response"
root_run_id=$(jq -er '.data.run_id' <<<"$root_response")
wait_for_root_run "$root_run_id" >/dev/null
wait_for_effect root 1
sleep 2
assert_root_effect_audit "$root_run_id"
if employee_ssh root curl --silent --fail http://127.0.0.1:7878/v1/runs >/dev/null 2>&1; then
    fail "root comparison employee reached run history without a token"
fi
employee_ssh root test ! -r /etc/omakure/tokens.toml

printf 'Fedora VM privilege certification: unprivileged service plus fixed Polkit gate\n'
assert_service_mode delegated omakure yes
delegated_response=$(dispatch_cue delegated certified-root-operation.sh 90)
jq -e '.ok == true and .data.accepted == true and .data.outcome_seen == true and .data.code == 0' \
    <<<"$delegated_response" >/dev/null || fail "delegated Cue was not accepted: $delegated_response"
wait_for_effect delegated 1
sleep 2
delegated_run_id=$(jq -er '.data.expected_run_id' <<<"$delegated_response")
assert_effect_audit delegated 1 "$delegated_run_id"
assert_employee_boundary

delegated_denied=$(dispatch_cue delegated unapproved.sh 5)
jq -e '.ok == true and .data.accepted == false and .data.code == 1206' \
    <<<"$delegated_denied" >/dev/null || fail "delegated Performer accepted an undeclared script: $delegated_denied"
[[ "$(effect_count delegated)" == 1 ]] || fail "rejected delegated Cue changed the effect count"
ssh_vm delegated sudo test ! -e /var/lib/omakure-workspace/unapproved-ran

printf 'Fedora VM privilege certification: revoke Conductor and prove fail-closed behavior\n'
node_cli delegated revoke "${node_ids[conductor]}" \
    --actor vm-certification --reason certification-complete --confirmed >/dev/null
if revoked_response=$(dispatch_cue delegated certified-root-operation.sh 5); then
    if jq -e '.ok == true and .data.accepted == true' <<<"$revoked_response" >/dev/null 2>&1; then
        fail "revoked Conductor dispatched another privileged operation: $revoked_response"
    fi
fi
sleep 3
[[ "$(effect_count delegated)" == 1 ]] || fail "revocation did not stop another privileged effect"

if [[ "$induced_failure" == after-certification ]]; then
    fail "induced failure after certification"
fi

printf 'Fedora VM privilege certification: passed; all VMs and run volumes will be removed\n'
