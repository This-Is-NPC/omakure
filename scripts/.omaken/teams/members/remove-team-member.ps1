# OMAKURE_SCHEMA_START
# {
#   "Name": "members_remove_team_member",
#   "Description": "Remove team member",
#   "Tags": ["teams", "members", "remove"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
#     },
#     {
#       "Name": "user_email",
#       "Type": "string",
#       "Required": true,
#       "Description": "User email address (UPN)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$UserEmail = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--user_email" { $UserEmail = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($UserEmail -eq "") { Write-Error "--user_email is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-teamuser?view=teams-ps
Remove-TeamUser -GroupId $GroupId -User $UserEmail
