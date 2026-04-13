# OMAKURE_SCHEMA_START
# {
#   "Name": "members_list_team_members",
#   "Description": "List team members",
#   "Tags": ["teams", "members", "list"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
#     },
#     {
#       "Name": "role",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Owner", "Member", "Guest"],
#       "Description": "Filter by role"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$Role = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--role" { $Role = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/get-teamuser?view=teams-ps
$params = @{
  GroupId = $GroupId
}
if ($Role -ne "") { $params["Role"] = $Role }

Get-TeamUser @params
