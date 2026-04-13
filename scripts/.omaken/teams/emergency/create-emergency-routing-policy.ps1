# OMAKURE_SCHEMA_START
# {
#   "Name": "emergency_create_routing_policy",
#   "Description": "Create emergency call routing policy",
#   "Tags": ["teams", "emergency", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "emergency_dial_string",
#       "Type": "string",
#       "Required": true,
#       "Default": "911",
#       "Description": "Emergency dial string"
#     },
#     {
#       "Name": "emergency_dial_mask",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Dial mask (e.g. 911;112)",
#       "Description": "Emergency dial mask"
#     },
#     {
#       "Name": "online_pstn_usage",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "PSTN usage for emergency",
#       "Description": "Online PSTN usage name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$EmergencyDialString = "911"
$EmergencyDialMask = ""
$OnlinePstnUsage = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--emergency_dial_string" { $EmergencyDialString = $args[++$i] }
    "--emergency_dial_mask" { $EmergencyDialMask = $args[++$i] }
    "--online_pstn_usage" { $OnlinePstnUsage = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($OnlinePstnUsage -eq "") { Write-Error "--online_pstn_usage is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsemergencycallroutingpolicy?view=teams-ps
$numberParams = @{
  EmergencyDialString = $EmergencyDialString
  OnlinePSTNUsage     = $OnlinePstnUsage
}
if ($EmergencyDialMask -ne "") { $numberParams["EmergencyDialMask"] = $EmergencyDialMask }

$emergencyNumber = New-CsTeamsEmergencyNumber @numberParams

New-CsTeamsEmergencyCallRoutingPolicy -Identity $Identity -EmergencyNumbers @($emergencyNumber)
