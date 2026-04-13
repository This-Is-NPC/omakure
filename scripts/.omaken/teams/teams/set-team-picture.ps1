# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_set_picture",
#   "Description": "Set team photo",
#   "Tags": ["teams", "configure"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
#     },
#     {
#       "Name": "image_path",
#       "Type": "string",
#       "Required": true,
#       "Description": "Path to the image file"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$ImagePath = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--image_path" { $ImagePath = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($ImagePath -eq "") { Write-Error "--image_path is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-teampicture?view=teams-ps
Set-TeamPicture -GroupId $GroupId -ImagePath $ImagePath
