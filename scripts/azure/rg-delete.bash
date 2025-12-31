#!/usr/bin/env bash
set -euo pipefail

# ------------------------------------------------------------
# Delete a Resource Group and all resources inside it
# ------------------------------------------------------------

if [[ "${SCHEMA_MODE:-}" == "1" ]]; then
  cat <<'JSON'
{
  "Name": "rg_delete",
  "Description": "Delete a resource group and all resources inside it.",
  "Fields": [
    {
      "Name": "resource_group",
      "Prompt": "Resource group name",
      "Type": "string",
      "Order": 1,
      "Required": true,
      "Arg": "--resource-group"
    },
    {
      "Name": "subscription_id",
      "Prompt": "Subscription id (optional)",
      "Type": "string",
      "Order": 2,
      "Required": false,
      "Arg": "--subscription-id"
    },
    {
      "Name": "confirm",
      "Prompt": "Confirm deletion (true/false)",
      "Type": "bool",
      "Order": 3,
      "Required": false,
      "Arg": "--confirm",
      "Default": "false"
    }
  ]
}
JSON
  exit 0
fi

run_az() {
  local err_file output
  err_file="$(mktemp)"
  if ! output="$(az "$@" 2>"${err_file}")"; then
    cat "${err_file}" >&2
    rm -f "${err_file}"
    return 1
  fi
  if [[ -s "${err_file}" ]]; then
    grep -Ev 'BrokenPipeError|Exception ignored on flushing sys.stdout' "${err_file}" >&2 || true
  fi
  rm -f "${err_file}"
  printf '%s' "${output}"
}

prompt_if_empty() {
  local var_name="$1"
  local label="$2"
  local value="${!var_name:-}"
  if [[ -z "${value}" ]]; then
    read -r -p "${label}: " value
    printf -v "${var_name}" '%s' "${value}"
  fi
}

prompt_confirm() {
  local answer
  while true; do
    read -r -p "Type 'delete' to confirm: " answer
    if [[ "${answer}" == "delete" ]]; then
      return 0
    fi
    printf "Confirmation did not match. Aborting.\n" >&2
    return 1
  done
}

SUB_ID_OVERRIDE=""
RESOURCE_GROUP=""
CONFIRM="false"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --subscription-id)
      if [[ -z "${2:-}" ]]; then
        printf "Missing value for --subscription-id\n" >&2
        exit 1
      fi
      SUB_ID_OVERRIDE="$2"
      shift 2
      ;;
    --resource-group)
      if [[ -z "${2:-}" ]]; then
        printf "Missing value for --resource-group\n" >&2
        exit 1
      fi
      RESOURCE_GROUP="$2"
      shift 2
      ;;
    --confirm)
      if [[ -n "${2:-}" && "${2:-}" != --* ]]; then
        CONFIRM="$2"
        shift 2
      else
        CONFIRM="true"
        shift 1
      fi
      ;;
    *)
      printf "Unknown arg: %s\n" "$1" >&2
      exit 1
      ;;
  esac
done

prompt_if_empty RESOURCE_GROUP "Resource group name"

if [[ -n "${SUB_ID_OVERRIDE}" ]]; then
  SUB_ID="${SUB_ID_OVERRIDE}"
else
  SUB_ID="$(run_az account show --query id -o tsv)"
fi

printf "Resources that will be deleted in resource group '%s':\n" "${RESOURCE_GROUP}"
if ! RESOURCES_JSON="$(run_az resource list --resource-group "${RESOURCE_GROUP}" --subscription "${SUB_ID}" -o json)"; then
  printf "Failed to list resources for resource group.\n" >&2
  exit 1
fi

if jq -e '. | length == 0' <<< "${RESOURCES_JSON}" > /dev/null; then
  printf "No resources found in resource group.\n"
else
  mapfile -t RESOURCE_LINES < <(
    jq -r '.[] | [.name, .type, .id] | @tsv' <<< "${RESOURCES_JSON}"
  )

  name_width=36
  type_width=48
  id_width=64

  make_line() {
    local len=$1
    local line
    printf -v line '%*s' "$len" ''
    printf '%s' "${line// /-}"
  }

  printf "%-${name_width}.${name_width}s  %-${type_width}.${type_width}s  %-${id_width}.${id_width}s\n" \
    "Resource" "Type" "Id"
  printf "%-${name_width}s  %-${type_width}s  %-${id_width}s\n" \
    "$(make_line "$name_width")" "$(make_line "$type_width")" "$(make_line "$id_width")"

  for line in "${RESOURCE_LINES[@]}"; do
    IFS=$'\t' read -r RES_NAME RES_TYPE RES_ID <<< "${line}"
    printf "%-${name_width}.${name_width}s  %-${type_width}.${type_width}s  %-${id_width}.${id_width}s\n" \
      "${RES_NAME}" "${RES_TYPE}" "${RES_ID}"
  done
fi

printf "You are about to delete resource group '%s' in subscription '%s'.\n" \
  "${RESOURCE_GROUP}" "${SUB_ID}"
printf "This operation deletes all resources inside the group.\n"

if [[ "${CONFIRM}" != "true" ]]; then
  prompt_confirm
fi

printf "Deleting resource group...\n"
run_az group delete --name "${RESOURCE_GROUP}" --subscription "${SUB_ID}" --yes
printf "Delete request completed.\n"
