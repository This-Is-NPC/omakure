# OMAKURE_SCHEMA_START
# {
#   "Name": "channels_remove_channel_member",
#   "Description": "Remove member from channel",
#   "Tags": ["teams", "channels", "members", "remove"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
#     },
#     {
#       "Name": "display_name",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Channel Name",
#       "Description": "Display name of the channel"
#     },
#     {
#       "Name": "user_email",
#       "Type": "string",
#       "Required": true,
#       "Description": "Email address of the user to remove"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$DisplayName = ""
$UserEmail = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--display_name" { $DisplayName = $args[++$i] }
    "--user_email" { $UserEmail = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($DisplayName -eq "") { Write-Error "--display_name is required"; exit 1 }
if ($UserEmail -eq "") { Write-Error "--user_email is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-teamchanneluser?view=teams-ps
Remove-TeamChannelUser -GroupId $GroupId -DisplayName $DisplayName -User $UserEmail
