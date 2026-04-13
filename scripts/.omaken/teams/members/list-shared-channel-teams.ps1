# OMAKURE_SCHEMA_START
# {
#   "Name": "members_list_shared_channel_teams",
#   "Description": "List shared channel teams",
#   "Tags": ["teams", "members", "list"],
#   "Fields": [
#     {
#       "Name": "host_team_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The host team ID"
#     },
#     {
#       "Name": "channel_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The channel ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$HostTeamId = ""
$ChannelId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--host_team_id" { $HostTeamId = $args[++$i] }
    "--channel_id" { $ChannelId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($HostTeamId -eq "") { Write-Error "--host_team_id is required"; exit 1 }
if ($ChannelId -eq "") { Write-Error "--channel_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/get-sharedwithteam?view=teams-ps
Get-SharedWithTeam -HostTeamId $HostTeamId -ChannelId $ChannelId
