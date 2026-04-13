# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_remove_team",
#   "Description": "Delete team",
#   "Tags": ["teams", "remove"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team to remove"
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

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-team?view=teams-ps
Remove-Team -GroupId $GroupId
