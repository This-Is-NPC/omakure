# OMAKURE_SCHEMA_START
# {
#   "Name": "channels_list_channel_members",
#   "Description": "List channel members",
#   "Tags": ["teams", "channels", "members", "list"],
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
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$DisplayName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--display_name" { $DisplayName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($DisplayName -eq "") { Write-Error "--display_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/get-teamchanneluser?view=teams-ps
Get-TeamChannelUser -GroupId $GroupId -DisplayName $DisplayName
