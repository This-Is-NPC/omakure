#!/usr/bin/env bash
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
command -v git >/dev/null 2>&1 || {
    printf 'hooks:install: git is required\n' >&2
    exit 1
}
[[ -d "$root/.git" || -f "$root/.git" ]] || {
    printf 'hooks:install: repository metadata is missing: %s\n' "$root/.git" >&2
    exit 1
}
[[ -x "$root/.githooks/pre-commit" && -x "$root/.githooks/pre-push" ]] || {
    printf 'hooks:install: tracked hooks are missing or not executable\n' >&2
    exit 1
}

git -C "$root" config --local core.hooksPath .githooks
printf 'hooks:install: configured local core.hooksPath=.githooks\n'
