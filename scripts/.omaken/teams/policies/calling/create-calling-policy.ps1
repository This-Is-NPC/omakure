# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_calling",
#   "Description": "Create calling policy",
#   "Tags": ["teams", "policy", "calling", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_private_calling",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow private calling"
#     },
#     {
#       "Name": "allow_voicemail",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["AlwaysEnabled", "AlwaysDisabled", "UserOverride"],
#       "Default": "AlwaysEnabled",
#       "Description": "Allow voicemail"
#     },
#     {
#       "Name": "allow_call_groups",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow call groups"
#     },
#     {
#       "Name": "allow_delegation",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow delegation"
#     },
#     {
#       "Name": "allow_call_forwarding_to_phone",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow call forwarding to phone"
#     },
#     {
#       "Name": "busy_on_busy",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled", "Unanswered"],
#       "Default": "Enabled",
#       "Description": "Busy on busy mode"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowPrivateCalling = "true"
$AllowVoicemail = "AlwaysEnabled"
$AllowCallGroups = "true"
$AllowDelegation = "true"
$AllowCallForwardingToPhone = "true"
$BusyOnBusy = "Enabled"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_private_calling" { $AllowPrivateCalling = $args[++$i] }
    "--allow_voicemail" { $AllowVoicemail = $args[++$i] }
    "--allow_call_groups" { $AllowCallGroups = $args[++$i] }
    "--allow_delegation" { $AllowDelegation = $args[++$i] }
    "--allow_call_forwarding_to_phone" { $AllowCallForwardingToPhone = $args[++$i] }
    "--busy_on_busy" { $BusyOnBusy = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamscallingpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowPrivateCalling -ne "") {
  $params["AllowPrivateCalling"] = if ($AllowPrivateCalling -eq "true") { $true } else { $false }
}
if ($AllowVoicemail -ne "") { $params["AllowVoicemail"] = $AllowVoicemail }
if ($AllowCallGroups -ne "") {
  $params["AllowCallGroups"] = if ($AllowCallGroups -eq "true") { $true } else { $false }
}
if ($AllowDelegation -ne "") {
  $params["AllowDelegation"] = if ($AllowDelegation -eq "true") { $true } else { $false }
}
if ($AllowCallForwardingToPhone -ne "") {
  $params["AllowCallForwardingToPhone"] = if ($AllowCallForwardingToPhone -eq "true") { $true } else { $false }
}
if ($BusyOnBusy -ne "") { $params["BusyOnBusyEnabledType"] = $BusyOnBusy }

New-CsTeamsCallingPolicy @params
