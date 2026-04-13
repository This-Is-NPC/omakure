# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_get_team",
#   "Description": "Get team by ID",
#   "Tags": ["teams", "list"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
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

# https://learn.microsoft.com/en-us/powershell/module/teams/get-team?view=teams-ps
Get-Team -GroupId $GroupId
