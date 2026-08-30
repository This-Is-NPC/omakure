#!/usr/bin/env bash
set -euo pipefail

root_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
fixture="$root_dir/.scripts/fixtures/fedora-vm-privilege"
main="$root_dir/.scripts/fedora-vm-privilege-certification.sh"
cleanup="$root_dir/.scripts/fedora-vm-privilege-certification-cleanup-test.sh"

bash -n \
    "$main" \
    "$cleanup" \
    "$fixture/guest-provision.sh" \
    "$fixture/battery/certified-root-operation.sh" \
    "$fixture/omakure-certified-root-operation"

rule=$(<"$fixture/50-omakure-certified-operation.rules")
[[ "$rule" == *'action.id == "org.freedesktop.systemd1.manage-units"'* ]]
[[ "$rule" == *'action.lookup("unit") == "omakure-certified-root-operation.service"'* ]]
[[ "$rule" == *'action.lookup("verb") == "start"'* ]]
[[ "$rule" == *'subject.user == "omakure"'* ]]
[[ "$rule" != *'polkit.Result.AUTH_ADMIN'* ]]

operation=$(<"$fixture/battery/certified-root-operation.sh")
[[ "$operation" == *'systemctl --no-ask-password start omakure-certified-root-operation.service'* ]]
[[ "$operation" != *'sudo '* && "$operation" != *'pkexec '* ]]

unit=$(<"$fixture/omakure-certified-root-operation.service")
[[ "$unit" == *'User=root'* ]]
[[ "$unit" == *'ExecStart=/usr/local/libexec/omakure-certified-root-operation'* ]]
[[ "$unit" == *'ProtectSystem=strict'* ]]
[[ "$unit" == *'ReadWritePaths=/var/lib/omakure-certified-root'* ]]

guest=$(<"$fixture/guest-provision.sh")
[[ "$guest" == *'User=root'* && "$guest" == *'NoNewPrivileges=false'* ]]
[[ "$guest" == *'omakure-root-api.service'* && "$guest" == *'omakure-root-worker.service'* ]]
[[ "$guest" != *'--node-state-dir'* && "$guest" != *'OMAKURE_NODE_TEST_MODE'* ]]
[[ "$guest" == *'/etc/polkit-1/rules.d/50-omakure-certified-operation.rules'* ]]
[[ "$guest" == *'battery install certified-privilege certified.root-operation'* ]]
[[ "$guest" == *'"$role" != root'* && "$guest" == *'/home/harness/.omakure-client-token'* ]]
[[ "$guest" == *'report_failure'* ]]

harness=$(<"$main")
cleanup=$(<"$cleanup")
[[ "$harness" == *'28680fe5b371a5a82ebf43a31926e086a168e59949d03969c5093e7071f90b7f'* ]]
[[ "$harness" == *'--target x86_64-unknown-linux-musl'* ]]
[[ "$harness" == *'CARGO_PROFILE_RELEASE_DEBUG=1'* ]]
[[ "$harness" == *'assert_service_mode delegated omakure yes'* ]]
[[ "$harness" == *'assert_service_mode root root no'* ]]
[[ "$harness" == *'OMAKURE_VM_CERTIFICATION_INDUCE_FAILURE'* ]]
[[ "$harness" == *'OMAKURE_VM_CERTIFICATION_INDUCE_INSPECTION_FAILURE'* ]]
[[ "$harness" == *'net-dhcp-leases "$network" --mac "$mac"'* ]]
[[ "$harness" == *'cloud-init status --long'* ]]
[[ "$harness" == *'coredumpctl -q -1'* ]]
[[ "$harness" == *'node_api_status "$role"'* ]]
[[ "$harness" == *'http://127.0.0.1:7878/v1/node/status'* ]]
[[ "$harness" == *'firewall-cmd --quiet --query-port=7879/tcp'* ]]
[[ "$harness" == *'firewall-cmd --quiet --query-port=7878/tcp'* ]]
[[ "$harness" == *'unable to inspect remaining domains'* ]]
[[ "$harness" == *'unable to inspect remaining volumes'* ]]
[[ "$harness" != *'list --all --name 2>/dev/null || true'* ]]
[[ "$harness" != *'vol-list --pool "$pool" 2>/dev/null || true'* ]]

[[ "$cleanup" == *'timeout --foreground --kill-after=5s 60s virsh -q -c "$uri"'* ]]
[[ "$cleanup" == *'OMAKURE_VM_CERTIFICATION_INDUCE_FAILURE=after-first-vm'* ]]
[[ "$cleanup" == *'assert_removed "a real induced failure"'* ]]
[[ "$cleanup" == *'unable to inspect domains after'* ]]
[[ "$cleanup" == *'unable to inspect volumes after'* ]]
[[ "$cleanup" == *'OMAKURE_VM_CERTIFICATION_INDUCE_INSPECTION_FAILURE=both'* ]]

printf 'Fedora VM privilege certification static test: passed\n'
