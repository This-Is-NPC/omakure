# OMAKURE_SCHEMA_START
# {
#   "Name": "channels_create_channel_policy",
#   "Description": "Create channels policy",
#   "Tags": ["teams", "channels", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Policy Name",
#       "Description": "Identity name for the policy"
#     },
#     {
#       "Name": "allow_private_channel_creation",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow private channel creation"
#     },
#     {
#       "Name": "allow_shared_channel_creation",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow shared channel creation"
#     },
#     {
#       "Name": "allow_org_wide_team_creation",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow org-wide team creation"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowPrivateChannelCreation = "true"
$AllowSharedChannelCreation = "true"
$AllowOrgWideTeamCreation = "true"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_private_channel_creation" { $AllowPrivateChannelCreation = $args[++$i] }
    "--allow_shared_channel_creation" { $AllowSharedChannelCreation = $args[++$i] }
    "--allow_org_wide_team_creation" { $AllowOrgWideTeamCreation = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# Convert string booleans to PowerShell booleans
$AllowPrivate = if ($AllowPrivateChannelCreation -eq "true") { $true } else { $false }
$AllowShared = if ($AllowSharedChannelCreation -eq "true") { $true } else { $false }
$AllowOrgWide = if ($AllowOrgWideTeamCreation -eq "true") { $true } else { $false }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamschannelspolicy?view=teams-ps
New-CsTeamsChannelsPolicy -Identity $Identity -AllowPrivateChannelCreation $AllowPrivate -AllowSharedChannelCreation $AllowShared -AllowOrgWideTeamCreation $AllowOrgWide
