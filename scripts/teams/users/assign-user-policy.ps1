# OMAKURE_SCHEMA_START
# {
#   "Name": "users_assign_policy",
#   "Description": "Assign policy to user",
#   "Tags": ["teams", "users", "policy", "assign"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email or UPN",
#       "Description": "User email or UPN"
#     },
#     {
#       "Name": "policy_type",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Policy Type",
#       "Choices": ["TeamsCallingPolicy", "TeamsMessagingPolicy", "TeamsMeetingPolicy", "TeamsAppSetupPolicy", "TeamsAppPermissionPolicy", "TeamsChannelsPolicy", "TeamsEmergencyCallingPolicy"],
#       "Description": "Type of policy to assign"
#     },
#     {
#       "Name": "policy_name",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Policy Name",
#       "Description": "Name of the policy to assign"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$PolicyType = ""
$PolicyName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--policy_type" { $PolicyType = $args[++$i] }
    "--policy_name" { $PolicyName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($PolicyType -eq "") { Write-Error "--policy_type is required"; exit 1 }
if ($PolicyName -eq "") { Write-Error "--policy_name is required"; exit 1 }

# Teams user policy assignment uses policy-specific Grant-* cmdlets.
$CommandName = switch ($PolicyType) {
  "TeamsCallingPolicy" { "Grant-CsTeamsCallingPolicy" }
  "TeamsMessagingPolicy" { "Grant-CsTeamsMessagingPolicy" }
  "TeamsMeetingPolicy" { "Grant-CsTeamsMeetingPolicy" }
  "TeamsAppSetupPolicy" { "Grant-CsTeamsAppSetupPolicy" }
  "TeamsAppPermissionPolicy" { "Grant-CsTeamsAppPermissionPolicy" }
  "TeamsChannelsPolicy" { "Grant-CsTeamsChannelsPolicy" }
  "TeamsEmergencyCallingPolicy" { "Grant-CsTeamsEmergencyCallingPolicy" }
  default {
    Write-Error "Unsupported policy type: $PolicyType"
    exit 1
  }
}

& $CommandName -Identity $Identity -PolicyName $PolicyName
