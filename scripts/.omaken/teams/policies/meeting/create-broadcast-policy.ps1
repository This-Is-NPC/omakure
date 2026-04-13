# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_broadcast",
#   "Description": "Create broadcast policy",
#   "Tags": ["teams", "policy", "meeting", "broadcast", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_broadcast_scheduling",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow broadcast scheduling"
#     },
#     {
#       "Name": "allow_broadcast_transcription",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Allow broadcast transcription"
#     },
#     {
#       "Name": "broadcast_attendee_visibility",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["EveryoneInCompany", "Everyone"],
#       "Default": "EveryoneInCompany",
#       "Description": "Broadcast attendee visibility"
#     },
#     {
#       "Name": "broadcast_recording_mode",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["AlwaysEnabled", "AlwaysDisabled", "UserOverride"],
#       "Default": "UserOverride",
#       "Description": "Broadcast recording mode"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowBroadcastScheduling = "true"
$AllowBroadcastTranscription = "false"
$BroadcastAttendeeVisibility = "EveryoneInCompany"
$BroadcastRecordingMode = "UserOverride"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_broadcast_scheduling" { $AllowBroadcastScheduling = $args[++$i] }
    "--allow_broadcast_transcription" { $AllowBroadcastTranscription = $args[++$i] }
    "--broadcast_attendee_visibility" { $BroadcastAttendeeVisibility = $args[++$i] }
    "--broadcast_recording_mode" { $BroadcastRecordingMode = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsmeetingbroadcastpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowBroadcastScheduling -ne "") {
  $params["AllowBroadcastScheduling"] = if ($AllowBroadcastScheduling -eq "true") { $true } else { $false }
}
if ($AllowBroadcastTranscription -ne "") {
  $params["AllowBroadcastTranscription"] = if ($AllowBroadcastTranscription -eq "true") { $true } else { $false }
}
if ($BroadcastAttendeeVisibility -ne "") { $params["BroadcastAttendeeVisibilityMode"] = $BroadcastAttendeeVisibility }
if ($BroadcastRecordingMode -ne "") { $params["BroadcastRecordingMode"] = $BroadcastRecordingMode }

New-CsTeamsMeetingBroadcastPolicy @params
