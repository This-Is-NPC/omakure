# OMAKURE_SCHEMA_START
# {
#   "Name": "channels_update_channel",
#   "Description": "Update channel",
#   "Tags": ["teams", "channels", "configure"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
#     },
#     {
#       "Name": "current_display_name",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Current Channel Name",
#       "Description": "Current display name of the channel"
#     },
#     {
#       "Name": "new_display_name",
#       "Type": "string",
#       "Required": false,
#       "Description": "New display name for the channel"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "New description for the channel"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$CurrentDisplayName = ""
$NewDisplayName = ""
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--current_display_name" { $CurrentDisplayName = $args[++$i] }
    "--new_display_name" { $NewDisplayName = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($CurrentDisplayName -eq "") { Write-Error "--current_display_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-teamchannel?view=teams-ps
$params = @{
  GroupId            = $GroupId
  CurrentDisplayName = $CurrentDisplayName
}
if ($NewDisplayName -ne "") { $params["NewDisplayName"] = $NewDisplayName }
if ($Description -ne "") { $params["Description"] = $Description }

Set-TeamChannel @params
