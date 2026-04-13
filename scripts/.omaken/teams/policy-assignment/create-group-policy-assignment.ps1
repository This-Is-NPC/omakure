# OMAKURE_SCHEMA_START
# {
#   "Name": "assignment_create_group",
#   "Description": "Create group policy assignment",
#   "Tags": ["teams", "policy", "group", "create"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "Group ID"
#     },
#     {
#       "Name": "policy_type",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["TeamsCallingPolicy", "TeamsMessagingPolicy", "TeamsMeetingPolicy", "TeamsAppSetupPolicy"],
#       "Description": "Policy type"
#     },
#     {
#       "Name": "policy_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "rank",
#       "Type": "string",
#       "Required": false,
#       "Default": "1",
#       "Description": "Assignment rank"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$PolicyType = ""
$PolicyName = ""
$Rank = "1"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--policy_type" { $PolicyType = $args[++$i] }
    "--policy_name" { $PolicyName = $args[++$i] }
    "--rank" { $Rank = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($PolicyType -eq "") { Write-Error "--policy_type is required"; exit 1 }
if ($PolicyName -eq "") { Write-Error "--policy_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csgrouppolicyassignment?view=teams-ps
$params = @{
  GroupId    = $GroupId
  PolicyType = $PolicyType
  PolicyName = $PolicyName
}
if ($Rank -ne "") { $params["Rank"] = [int]$Rank }

New-CsGroupPolicyAssignment @params
