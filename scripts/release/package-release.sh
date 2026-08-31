#!/usr/bin/env bash
set -euo pipefail

binary="${1:?binary path is required}"
archive="${2:?archive path is required}"
binary_name="$(basename "${binary}")"

test -f "${binary}"
mkdir -p "$(dirname "${archive}")"
rm -f "${archive}"

case "${archive}" in
  *.tar.gz)
    tar -czf "${archive}" -C "$(dirname "${binary}")" "${binary_name}"
    test "$(tar -tzf "${archive}")" = "${binary_name}"
    ;;
  *)
    printf 'unsupported archive format: %s\n' "${archive}" >&2
    exit 2
    ;;
esac
