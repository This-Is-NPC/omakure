# OMAKURE_SCHEMA_START
# {
#   "Name": "channels_create_channel",
#   "Description": "Create channel",
#   "Tags": ["teams", "channels", "create"],
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
#       "Description": "Display name for the channel"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description for the channel"
#     },
#     {
#       "Name": "membership_type",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Standard", "Private", "Shared"],
#       "Default": "Standard",
#       "Description": "Channel membership type"
#     },
#     {
#       "Name": "owner",
#       "Type": "string",
#       "Required": false,
#       "Description": "Owner UPN or ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$DisplayName = ""
$Description = ""
$MembershipType = "Standard"
$Owner = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--display_name" { $DisplayName = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--membership_type" { $MembershipType = $args[++$i] }
    "--owner" { $Owner = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($DisplayName -eq "") { Write-Error "--display_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-teamchannel?view=teams-ps
$params = @{
  GroupId        = $GroupId
  DisplayName    = $DisplayName
  MembershipType = $MembershipType
}
if ($Description -ne "") { $params["Description"] = $Description }
if ($Owner -ne "") { $params["Owner"] = $Owner }

New-TeamChannel @params
