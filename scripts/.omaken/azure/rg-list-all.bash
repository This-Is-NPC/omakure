#!/usr/bin/env bash

# ------------------------------------------------------------
# List Resource Groups with CreatedAt / LastModified / CreatedBy
# Uses Azure Activity Log (last 90 days limitation applies)
# ------------------------------------------------------------

# OMAKURE_SCHEMA_START
# {
#   "Name": "rg_list_all",
#   "Description": "List resource groups with CreatedAt, LastModified, and CreatedBy.",
#   "Fields": [
#     {
#       "Name": "subscription_id",
#       "Prompt": "Subscription id (optional)",
#       "Type": "string",
#       "Order": 1,
#       "Required": false,
#       "Arg": "--subscription-id"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

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

SUB_ID_OVERRIDE=""
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
    *)
      printf "Unknown arg: %s\n" "$1" >&2
      exit 1
      ;;
  esac
done

# Get subscription ID once
if [[ -n "${SUB_ID_OVERRIDE}" ]]; then
  SUB_ID="${SUB_ID_OVERRIDE}"
else
  SUB_ID="$(run_az account show --query id -o tsv)"
fi

# Get all Resource Group names + ids (avoid Bash's special GROUPS variable)
mapfile -t RG_LINES < <(run_az group list --query "[].{name:name,id:id}" -o tsv)

printf "Script started at: %s\n\n" "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

rg_width=48
created_width=20
modified_width=20
created_by_width=36

make_line() {
  local len=$1
  local line
  printf -v line '%*s' "$len" ''
  printf '%s' "${line// /-}"
}

printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
  "ResourceGroup" "CreatedAt" "LastModified" "CreatedBy"
printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
  "$(make_line "$rg_width")" "$(make_line "$created_width")" \
  "$(make_line "$modified_width")" "$(make_line "$created_by_width")"

for line in "${RG_LINES[@]}"; do
  IFS=$'\t' read -r RG_NAME RG_ID <<< "${line}"
  if [[ -n "${RG_ID}" ]]; then
    RESOURCE_ID="${RG_ID}"
  else
    RESOURCE_ID="/subscriptions/${SUB_ID}/resourceGroups/${RG_NAME}"
  fi

  # Query Activity Log (stderr suppressed to avoid Azure CLI BrokenPipeError noise)
  if ! EVENTS="$(run_az monitor activity-log list \
    --resource-id "${RESOURCE_ID}" \
    --status Succeeded \
    --offset 90d \
    -o json)"; then
    printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
      "${RG_NAME}" "N/A" "N/A" "N/A"
    continue
  fi

  # Extract timestamps and caller
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
    ' <<< "${EVENTS}"
  )

  printf "%-${rg_width}s  %-${created_width}s  %-${modified_width}s  %-${created_by_width}s\n" \
    "${RG_NAME}" "${CREATED_AT}" "${LAST_MODIFIED}" "${CREATED_BY}"
done

printf "\nScript finished at: %s\n" "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
