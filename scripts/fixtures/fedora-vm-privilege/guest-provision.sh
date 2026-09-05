#!/usr/bin/env bash
set -euo pipefail

role=${1:?usage: guest-provision.sh conductor|root|delegated VERSION}
version=${2:?usage: guest-provision.sh conductor|root|delegated VERSION}
case "$role" in
    conductor|root|delegated) ;;
    *)
        printf 'guest provision: invalid role %s\n' "$role" >&2
        exit 2
        ;;
esac

staging=/home/harness/vm-cert
fixture="$staging/fixture"
artifact="$staging/omakure.tar.gz"
tokens="$staging/tokens.toml"

report_failure() {
    local status=$1 line=$2 file
    trap - ERR
    printf 'guest provision: role=%s failed at line=%s status=%s\n' \
        "$role" "$line" "$status" >&2
    for file in /home/harness/battery-{add,sync,install}.json; do
        if [[ -s "$file" ]]; then
            printf 'guest provision: %s\n' "${file##*/}" >&2
            jq -c '{ok, error, data: (.data | if type == "object" then with_entries(select(.key != "token" and .key != "tokens_file_entry")) else . end)}' \
                "$file" >&2 || true
        fi
    done
    exit "$status"
}
trap 'report_failure $? $LINENO' ERR

sudo env \
    ARTIFACT="$artifact" \
    bash "$staging/install.sh" \
    --artifact "$artifact" \
    --version "$version" \
    --install-node-service \
    --node-tokens-file "$tokens"

sudo systemctl stop omakure-node.service >/dev/null 2>&1 || true
sudo install -d -o root -g root -m 0700 /var/lib/omakure-certified-root
sudo install -d -o root -g root -m 0755 /usr/local/libexec
sudo install -o root -g root -m 0755 \
    "$fixture/omakure-certified-root-operation" \
    /usr/local/libexec/omakure-certified-root-operation
sudo install -o root -g root -m 0644 \
    "$fixture/omakure-certified-root-operation.service" \
    /etc/systemd/system/omakure-certified-root-operation.service

if [[ "$role" == root ]]; then
    sudo systemctl disable omakure-node.service >/dev/null
    sudo tee /etc/systemd/system/omakure-root-api.service >/dev/null <<'UNIT'
[Unit]
Description=Omakure VM certification root comparison API
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=root
Group=root
Environment=OMAKURE_TOKENS_FILE=/etc/omakure/tokens.toml
NoNewPrivileges=false
ExecStart=/usr/local/bin/omakure --scripts-dir /var/lib/omakure-workspace api --bind 0.0.0.0:7878 --allow-non-loopback --tokens-file /etc/omakure/tokens.toml
Restart=on-failure
PrivateTmp=true

[Install]
WantedBy=multi-user.target
UNIT
    sudo tee /etc/systemd/system/omakure-root-worker.service >/dev/null <<'UNIT'
[Unit]
Description=Omakure VM certification root comparison worker
After=omakure-root-api.service
Requires=omakure-root-api.service

[Service]
Type=simple
User=root
Group=root
NoNewPrivileges=false
ExecStart=/usr/local/bin/omakure --scripts-dir /var/lib/omakure-workspace queue worker --concurrency 1
Restart=on-failure
PrivateTmp=true

[Install]
WantedBy=multi-user.target
UNIT
else
    sudo install -d -o root -g root -m 0755 /etc/systemd/system/omakure-node.service.d
    sudo tee /etc/systemd/system/omakure-node.service.d/50-vm-certification.conf >/dev/null <<'UNIT'
[Service]
ExecStart=
ExecStart=/usr/local/bin/omakure node serve --workers 1 --no-scheduler --allow-non-loopback-direct
UNIT
fi

if [[ "$role" == delegated ]]; then
    sudo install -o root -g root -m 0644 \
        "$fixture/50-omakure-certified-operation.rules" \
        /etc/polkit-1/rules.d/50-omakure-certified-operation.rules
    sudo systemctl restart polkit.service
fi

if [[ "$role" != conductor ]]; then
    # Build a disposable external Battery repository as test input. Subject
    # scripts belong to their Battery repository, never to this fixture.
    battery_source=/var/lib/omakure-battery-source
    sudo install -d -o omakure -g omakure -m 0750 "$battery_source"
    sudo tee "$battery_source/omakure-battery.toml" >/dev/null <<'TOML'
[battery]
name = "certified-privilege"
version = "0.1.0"
description = "Synthetic Battery used only by the Fedora VM certification"

[[scripts]]
id = "certified.root-operation"
path = "certified-root-operation.sh"
description = "Ask systemd to run the one root operation authorized by local policy"
tags = ["certification", "privilege"]
TOML
    sudo tee "$battery_source/certified-root-operation.sh" >/dev/null <<'SCRIPT'
#!/usr/bin/env bash
set -euo pipefail
# OMAKURE_SCHEMA_START
# {"Name":"Certified root operation","Description":"Run the one root operation authorized by the host policy","Tags":["certification","privilege"],"Fields":[]}
# OMAKURE_SCHEMA_END
systemctl --no-ask-password start omakure-certified-root-operation.service
SCRIPT
    sudo chown -R omakure:omakure "$battery_source"
    sudo chmod 0750 "$battery_source/certified-root-operation.sh"
    sudo -u omakure git -C "$battery_source" init -b main >/dev/null
    sudo -u omakure git -C "$battery_source" add .
    sudo -u omakure git -C "$battery_source" \
        -c user.name='VM Certification' \
        -c user.email='certification@example.invalid' \
        commit -m 'synthetic battery fixture' >/dev/null

    sudo -u omakure env HOME=/var/lib/omakure-workspace \
        /usr/local/bin/omakure --scripts-dir /var/lib/omakure-workspace --json \
        battery add "$battery_source" --name certified-privilege --ref main \
        >/home/harness/battery-add.json
    sudo -u omakure env HOME=/var/lib/omakure-workspace \
        /usr/local/bin/omakure --scripts-dir /var/lib/omakure-workspace --json \
        battery sync certified-privilege >/home/harness/battery-sync.json
    sudo -u omakure env HOME=/var/lib/omakure-workspace \
        /usr/local/bin/omakure --scripts-dir /var/lib/omakure-workspace --json \
        battery install certified-privilege certified.root-operation \
        >/home/harness/battery-install.json
    jq -e '.ok == true' /home/harness/battery-add.json >/dev/null
    jq -e '.ok == true' /home/harness/battery-sync.json >/dev/null
    jq -e '.ok == true and .data.battery_name == "certified-privilege"' \
        /home/harness/battery-install.json >/dev/null

    sudo tee /var/lib/omakure-workspace/unapproved.sh >/dev/null <<'SCRIPT'
#!/usr/bin/env bash
# OMAKURE_SCHEMA_START
# {"Name":"Unapproved operation","Description":"Must never run from a Cue","Fields":[]}
# OMAKURE_SCHEMA_END
touch /var/lib/omakure-workspace/unapproved-ran
SCRIPT
    sudo chown omakure:omakure /var/lib/omakure-workspace/unapproved.sh
    sudo chmod 0750 /var/lib/omakure-workspace/unapproved.sh
fi

if [[ "$role" != root ]]; then
    install -o harness -g harness -m 0600 \
        "$staging/client.token" /home/harness/.omakure-client-token
fi

if [[ "$role" == conductor ]]; then
    install -o harness -g harness -m 0600 \
        "$staging/root.client.token" /home/harness/.root-client-token
fi

if [[ "$role" == root ]]; then
    # The comparison deliberately gives the API and worker the whole workspace
    # as root. The shipped node service remains tied to the omakure principal.
    sudo chown -R root:root /var/lib/omakure-workspace
    sudo chmod 0750 /var/lib/omakure-workspace
fi

sudo systemctl daemon-reload
if [[ "$role" == root ]]; then
    sudo systemctl enable omakure-root-api.service omakure-root-worker.service >/dev/null
fi
sudo rm -f "$tokens"
rm -f "$artifact" "$staging/client.token" "$staging/root.client.token"
trap - ERR
