# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_create_team",
#   "Description": "Create team",
#   "Tags": ["teams", "create"],
#   "Fields": [
#     {
#       "Name": "display_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Display name for the team"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description for the team"
#     },
#     {
#       "Name": "visibility",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Public", "Private"],
#       "Default": "Private",
#       "Description": "Team visibility"
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

$DisplayName = ""
$Description = ""
$Visibility = "Private"
$Owner = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--display_name" { $DisplayName = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--visibility" { $Visibility = $args[++$i] }
    "--owner" { $Owner = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($DisplayName -eq "") { Write-Error "--display_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-team?view=teams-ps
$params = @{
  DisplayName = $DisplayName
  Visibility  = $Visibility
}
if ($Description -ne "") { $params["Description"] = $Description }
if ($Owner -ne "") { $params["Owner"] = $Owner }

New-Team @params
