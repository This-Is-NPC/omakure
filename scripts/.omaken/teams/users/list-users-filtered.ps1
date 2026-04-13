# OMAKURE_SCHEMA_START
# {
#   "Name": "users_list_users_filtered",
#   "Description": "List users by filter",
#   "Tags": ["teams", "users", "list"],
#   "Fields": [
#     {
#       "Name": "filter",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Filter expression (e.g. Department -eq 'Engineering')",
#       "Description": "Filter expression for user search"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Filter = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--filter" { $Filter = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Filter -eq "") { Write-Error "--filter is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/get-csonlineuser?view=teams-ps
Get-CsOnlineUser -Filter $Filter
