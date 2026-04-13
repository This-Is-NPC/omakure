# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_meeting",
#   "Description": "Create meeting policy",
#   "Tags": ["teams", "policy", "meeting", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_channel_meeting_scheduling",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow channel meeting scheduling"
#     },
#     {
#       "Name": "allow_meet_now",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow Meet Now"
#     },
#     {
#       "Name": "allow_recording",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow cloud recording"
#     },
#     {
#       "Name": "allow_transcription",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Allow transcription"
#     },
#     {
#       "Name": "allow_ip_video",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow IP video"
#     },
#     {
#       "Name": "screen_sharing_mode",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["EntireScreen", "SingleApplication", "Disabled"],
#       "Default": "EntireScreen",
#       "Description": "Screen sharing mode"
#     },
#     {
#       "Name": "auto_admitted_users",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["EveryoneInCompany", "Everyone", "EveryoneInSameAndFederatedCompany", "OrganizerOnly"],
#       "Default": "EveryoneInCompany",
#       "Description": "Auto admitted users"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowChannelMeetingScheduling = "true"
$AllowMeetNow = "true"
$AllowRecording = "true"
$AllowTranscription = "false"
$AllowIPVideo = "true"
$ScreenSharingMode = "EntireScreen"
$AutoAdmittedUsers = "EveryoneInCompany"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_channel_meeting_scheduling" { $AllowChannelMeetingScheduling = $args[++$i] }
    "--allow_meet_now" { $AllowMeetNow = $args[++$i] }
    "--allow_recording" { $AllowRecording = $args[++$i] }
    "--allow_transcription" { $AllowTranscription = $args[++$i] }
    "--allow_ip_video" { $AllowIPVideo = $args[++$i] }
    "--screen_sharing_mode" { $ScreenSharingMode = $args[++$i] }
    "--auto_admitted_users" { $AutoAdmittedUsers = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsmeetingpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowChannelMeetingScheduling -ne "") {
  $params["AllowChannelMeetingScheduling"] = if ($AllowChannelMeetingScheduling -eq "true") { $true } else { $false }
}
if ($AllowMeetNow -ne "") {
  $params["AllowMeetNow"] = if ($AllowMeetNow -eq "true") { $true } else { $false }
}
if ($AllowRecording -ne "") {
  $params["AllowCloudRecording"] = if ($AllowRecording -eq "true") { $true } else { $false }
}
if ($AllowTranscription -ne "") {
  $params["AllowTranscription"] = if ($AllowTranscription -eq "true") { $true } else { $false }
}
if ($AllowIPVideo -ne "") {
  $params["AllowIPVideo"] = if ($AllowIPVideo -eq "true") { $true } else { $false }
}
if ($ScreenSharingMode -ne "") { $params["ScreenSharingMode"] = $ScreenSharingMode }
if ($AutoAdmittedUsers -ne "") { $params["AutoAdmittedUsers"] = $AutoAdmittedUsers }

New-CsTeamsMeetingPolicy @params
