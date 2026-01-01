#!/usr/bin/env bash
set -euo pipefail

# ------------------------------------------------------------
# Show Resource Group details + list resources with audit info
# Uses Azure Activity Log (last 90 days limitation applies)
# ------------------------------------------------------------

if [[ "${SCHEMA_MODE:-}" == "1" ]]; then
  cat <<'JSON'
{
  "Name": "rg_details",
  "Description": "Show resource group details and list resources with CreatedAt, LastModified, and CreatedBy.",
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

make_line() {
  local len=$1
  local line
  printf -v line '%*s' "$len" ''
  printf '%s' "${line// /-}"
}

SUB_ID_OVERRIDE=""
RESOURCE_GROUP=""

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

printf "Script started at: %s\n\n" "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
printf "Note: Activity Log only covers the last 90 days.\n\n"

printf "Resource Group details for: %s\n" "${RESOURCE_GROUP}"
if ! RG_JSON="$(run_az group show --name "${RESOURCE_GROUP}" --subscription "${SUB_ID}" -o json)"; then
  printf "Failed to fetch resource group details.\n" >&2
  exit 1
fi
RG_RESOURCE_ID="$(jq -r '.id // empty' <<< "${RG_JSON}")"
if [[ -z "${RG_RESOURCE_ID}" ]]; then
  RG_RESOURCE_ID="/subscriptions/${SUB_ID}/resourceGroups/${RESOURCE_GROUP}"
fi
jq . <<< "${RG_JSON}"
printf "\n"

rg_width=48
created_width=20
modified_width=20
created_by_width=36

printf "Resource Group audit\n"
printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
  "ResourceGroup" "CreatedAt" "LastModified" "CreatedBy"
printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
  "$(make_line "$rg_width")" "$(make_line "$created_width")" \
  "$(make_line "$modified_width")" "$(make_line "$created_by_width")"

if ! RG_EVENTS="$(run_az monitor activity-log list \
  --resource-id "${RG_RESOURCE_ID}" \
  --status Succeeded \
  --offset 90d \
  -o json)"; then
  printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
    "${RESOURCE_GROUP}" "N/A" "N/A" "N/A"
else
  read -r CREATED_AT LAST_MODIFIED CREATED_BY < <(
    jq -r '
      map(select((.operationName.value // "" | ascii_downcase) == "microsoft.resources/subscriptions/resourcegroups/write"))
      | sort_by(.eventTimestamp)
      | if length == 0 then
          ["N/A","N/A","N/A"]
        else
          [
            .[0].eventTimestamp // "N/A",
            .[-1].eventTimestamp // "N/A",
            (
              .[0].caller
              // .[0].claims."http://schemas.xmlsoap.org/ws/2005/05/identity/claims/emailaddress"
              // .[0].claims."http://schemas.xmlsoap.org/ws/2005/05/identity/claims/upn"
              // "N/A"
            )
          ]
        end
      | @tsv
    ' <<< "${RG_EVENTS}"
  )

  printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
    "${RESOURCE_GROUP}" "${CREATED_AT}" "${LAST_MODIFIED}" "${CREATED_BY}"
fi
printf "\n"

if ! RESOURCES_JSON="$(run_az resource list --resource-group "${RESOURCE_GROUP}" --subscription "${SUB_ID}" -o json)"; then
  printf "Failed to list resources for resource group.\n" >&2
  exit 1
fi

if jq -e '. | length == 0' <<< "${RESOURCES_JSON}" > /dev/null; then
  printf "No resources found in resource group.\n"
  printf "\nScript finished at: %s\n" "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
  exit 0
fi

mapfile -t RESOURCE_LINES < <(
  jq -r '.[] | [.name, .type, .id] | @tsv' <<< "${RESOURCES_JSON}"
)

name_width=36
type_width=48
printf "%-${name_width}.${name_width}s  %-${type_width}.${type_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
  "Resource" "Type" "CreatedAt" "LastModified" "CreatedBy"
printf "%-${name_width}s  %-${type_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
  "$(make_line "$name_width")" "$(make_line "$type_width")" \
  "$(make_line "$created_width")" "$(make_line "$modified_width")" "$(make_line "$created_by_width")"

for line in "${RESOURCE_LINES[@]}"; do
  IFS=$'\t' read -r RES_NAME RES_TYPE RES_ID <<< "${line}"

  if ! EVENTS="$(run_az monitor activity-log list \
    --resource-id "${RES_ID}" \
    --status Succeeded \
    -o json)"; then
    printf "%-${name_width}.${name_width}s  %-${type_width}.${type_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
      "${RES_NAME}" "${RES_TYPE}" "N/A" "N/A" "N/A"
    continue
  fi

  read -r CREATED_AT LAST_MODIFIED CREATED_BY < <(
    jq -r '
      sort_by(.eventTimestamp)
      | if length == 0 then
          ["N/A","N/A","N/A"]
        else
          [
            .[0].eventTimestamp // "N/A",
            .[-1].eventTimestamp // "N/A",
            .[0].caller // "N/A"
          ]
        end
      | @tsv
    ' <<< "${EVENTS}"
  )

  printf "%-${name_width}.${name_width}s  %-${type_width}.${type_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
    "${RES_NAME}" "${RES_TYPE}" "${CREATED_AT}" "${LAST_MODIFIED}" "${CREATED_BY}"
done

printf "\nScript finished at: %s\n" "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
