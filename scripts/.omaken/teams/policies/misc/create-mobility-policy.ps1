# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_mobility",
#   "Description": "Create mobility policy",
#   "Tags": ["teams", "policy", "mobility", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "ip_video_mobile",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["AllowAllNetworks", "AllowWiFiOnly"],
#       "Default": "AllowAllNetworks",
#       "Description": "IP video mobile network setting"
#     },
#     {
#       "Name": "ip_audio_mobile",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["AllowAllNetworks", "AllowWiFiOnly"],
#       "Default": "AllowAllNetworks",
#       "Description": "IP audio mobile network setting"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$IPVideoMobile = "AllowAllNetworks"
$IPAudioMobile = "AllowAllNetworks"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--ip_video_mobile" { $IPVideoMobile = $args[++$i] }
    "--ip_audio_mobile" { $IPAudioMobile = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsmobilitypolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($IPVideoMobile -ne "") { $params["IPVideoMobileMode"] = $IPVideoMobile }
if ($IPAudioMobile -ne "") { $params["IPAudioMobileMode"] = $IPAudioMobile }

New-CsTeamsMobilityPolicy @params
