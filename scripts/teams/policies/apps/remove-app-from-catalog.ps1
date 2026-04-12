# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_remove_app",
#   "Description": "Remove app from catalog",
#   "Tags": ["teams", "apps", "remove"],
#   "Fields": [
#     {
#       "Name": "app_id",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "App ID to remove",
#       "Description": "App ID to remove"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$AppId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--app_id" { $AppId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($AppId -eq "") { Write-Error "--app_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-teamsapp?view=teams-ps
Remove-TeamsApp -Id $AppId
