# OMAKURE_SCHEMA_START
# {
#   "Name": "conf_create_audio_policy",
#   "Description": "Create audio conferencing policy",
#   "Tags": ["teams", "conferencing", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_toll_free_dialin",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow toll-free dial-in"
#     },
#     {
#       "Name": "meeting_invite_phone_numbers",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Phone numbers (comma-separated)",
#       "Description": "Meeting invite phone numbers"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowTollFreeDialin = "true"
$MeetingInvitePhoneNumbers = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_toll_free_dialin" { $AllowTollFreeDialin = $args[++$i] }
    "--meeting_invite_phone_numbers" { $MeetingInvitePhoneNumbers = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsaudioconferencingpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowTollFreeDialin -ne "") {
  $params["AllowTollFreeDialin"] = if ($AllowTollFreeDialin -eq "true") { $true } else { $false }
}
if ($MeetingInvitePhoneNumbers -ne "") {
  $PhoneArray = $MeetingInvitePhoneNumbers -split ","
  $params["MeetingInvitePhoneNumbers"] = @($PhoneArray)
}

New-CsTeamsAudioConferencingPolicy @params
