# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_grant_misc",
#   "Description": "Grant miscellaneous policy",
#   "Tags": ["teams", "policy", "grant"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email",
#       "Description": "User email"
#     },
#     {
#       "Name": "policy_type",
#       "Type": "string",
#       "Required": true,
#       "Choices": [
#         "TeamsEnhancedEncryptionPolicy",
#         "TeamsEventsPolicy",
#         "TeamsIPPhonePolicy",
#         "TeamsUpdateManagementPolicy",
#         "TeamsAIPolicy",
#         "TeamsFeedbackPolicy",
#         "TeamsMobilityPolicy",
#         "TeamsVdiPolicy",
#         "TeamsVirtualAppointmentsPolicy",
#         "TeamsVoiceApplicationsPolicy"
#       ],
#       "Description": "Policy type"
#     },
#     {
#       "Name": "policy_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
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

# https://learn.microsoft.com/en-us/powershell/module/teams/?view=teams-ps
switch ($PolicyType) {
  "TeamsEnhancedEncryptionPolicy" {
    Grant-CsTeamsEnhancedEncryptionPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsEventsPolicy" {
    Grant-CsTeamsEventsPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsIPPhonePolicy" {
    Grant-CsTeamsIPPhonePolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsUpdateManagementPolicy" {
    Grant-CsTeamsUpdateManagementPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsAIPolicy" {
    Grant-CsTeamsAIPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsFeedbackPolicy" {
    Grant-CsTeamsFeedbackPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsMobilityPolicy" {
    Grant-CsTeamsMobilityPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsVdiPolicy" {
    Grant-CsTeamsVdiPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsVirtualAppointmentsPolicy" {
    Grant-CsTeamsVirtualAppointmentsPolicy -Identity $Identity -PolicyName $PolicyName
  }
  "TeamsVoiceApplicationsPolicy" {
    Grant-CsTeamsVoiceApplicationsPolicy -Identity $Identity -PolicyName $PolicyName
  }
  default {
    Write-Error "Unknown policy type: $PolicyType"; exit 1
  }
}
