# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_create_team_from_group",
#   "Description": "Create from M365 group",
#   "Tags": ["teams", "create"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The M365 group ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-team?view=teams-ps
New-Team -GroupId $GroupId
