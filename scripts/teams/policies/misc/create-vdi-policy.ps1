# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_vdi",
#   "Description": "Create VDI policy",
#   "Tags": ["teams", "policy", "vdi", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "disable_calls_and_meetings",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Disable calls and meetings"
#     },
#     {
#       "Name": "disable_av_in_calls",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Disable audio/video in calls"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$DisableCallsAndMeetings = "false"
$DisableAVInCalls = "false"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--disable_calls_and_meetings" { $DisableCallsAndMeetings = $args[++$i] }
    "--disable_av_in_calls" { $DisableAVInCalls = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsvdipolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($DisableCallsAndMeetings -ne "") {
  $params["DisableCallsAndMeetings"] = if ($DisableCallsAndMeetings -eq "true") { $true } else { $false }
}
if ($DisableAVInCalls -ne "") {
  $params["DisableAudioVideoInCallsAndMeetings"] = if ($DisableAVInCalls -eq "true") { $true } else { $false }
}

New-CsTeamsVdiPolicy @params
