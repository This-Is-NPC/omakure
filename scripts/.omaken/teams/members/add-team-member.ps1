# OMAKURE_SCHEMA_START
# {
#   "Name": "members_add_team_member",
#   "Description": "Add team member",
#   "Tags": ["teams", "members", "add"],
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
#     },
#     {
#       "Name": "role",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Member", "Owner"],
#       "Default": "Member",
#       "Description": "Role to assign"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$UserEmail = ""
$Role = "Member"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--user_email" { $UserEmail = $args[++$i] }
    "--role" { $Role = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($UserEmail -eq "") { Write-Error "--user_email is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/add-teamuser?view=teams-ps
Add-TeamUser -GroupId $GroupId -User $UserEmail -Role $Role
