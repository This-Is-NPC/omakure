# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_update_settings",
#   "Description": "Update team settings",
#   "Tags": ["teams", "configure"],
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
#       "Required": false,
#       "Description": "New display name"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "New description"
#     },
#     {
#       "Name": "visibility",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Public", "Private"],
#       "Description": "Team visibility"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$DisplayName = ""
$Description = ""
$Visibility = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--display_name" { $DisplayName = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--visibility" { $Visibility = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-team?view=teams-ps
$params = @{
  GroupId = $GroupId
}
if ($DisplayName -ne "") { $params["DisplayName"] = $DisplayName }
if ($Description -ne "") { $params["Description"] = $Description }
if ($Visibility -ne "") { $params["Visibility"] = $Visibility }

Set-Team @params
