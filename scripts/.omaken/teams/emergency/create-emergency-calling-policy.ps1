# OMAKURE_SCHEMA_START
# {
#   "Name": "emergency_create_calling_policy",
#   "Description": "Create emergency calling policy",
#   "Tags": ["teams", "emergency", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "notification_group",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Email for emergency notifications",
#       "Description": "Notification group email"
#     },
#     {
#       "Name": "notification_mode",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["NotificationOnly", "ConferenceMuted", "ConferenceUnMuted"],
#       "Default": "NotificationOnly",
#       "Description": "Notification mode"
#     },
#     {
#       "Name": "notification_dial_out_number",
#       "Type": "string",
#       "Required": false,
#       "Description": "Notification dial-out number"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$NotificationGroup = ""
$NotificationMode = "NotificationOnly"
$NotificationDialOutNumber = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--notification_group" { $NotificationGroup = $args[++$i] }
    "--notification_mode" { $NotificationMode = $args[++$i] }
    "--notification_dial_out_number" { $NotificationDialOutNumber = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($NotificationGroup -eq "") { Write-Error "--notification_group is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsemergencycallingpolicy?view=teams-ps
$params = @{
  Identity          = $Identity
  NotificationGroup = $NotificationGroup
}
if ($NotificationMode -ne "") { $params["NotificationMode"] = $NotificationMode }
if ($NotificationDialOutNumber -ne "") { $params["NotificationDialOutNumber"] = $NotificationDialOutNumber }

New-CsTeamsEmergencyCallingPolicy @params
