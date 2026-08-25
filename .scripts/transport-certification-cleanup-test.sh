#!/usr/bin/env bash
set -euo pipefail

project="omakure-transport-certification-induced-${BASHPID}"
if OMAKURE_CERTIFICATION_PROJECT="$project" OMAKURE_CERTIFICATION_INDUCE_FAILURE=1 \
    OMAKURE_CERTIFICATION_SKIP_RETAINED=1 \
    timeout --foreground --kill-after=10s 5m ./.scripts/transport-certification.sh; then
    printf 'transport certification cleanup test: induced failure unexpectedly passed\n' >&2
    exit 1
fi

for resource in container network volume; do
    if [[ -n "$(timeout --foreground --kill-after=5s 30s docker "$resource" ls -q \
        --filter "label=com.docker.compose.project=$project")" ]]; then
        printf 'transport certification cleanup test: %s survived for %s\n' "$resource" "$project" >&2
        exit 1
    fi
done
printf 'transport certification cleanup test: passed\n'
