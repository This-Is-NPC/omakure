# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_call_hold",
#   "Description": "Create call hold policy",
#   "Tags": ["teams", "policy", "calling", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Policy description"
#     },
#     {
#       "Name": "audio_file_id",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Audio File ID for hold music",
#       "Description": "Audio file ID for hold music"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Description = ""
$AudioFileId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--audio_file_id" { $AudioFileId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamscallholdpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($Description -ne "") { $params["Description"] = $Description }
if ($AudioFileId -ne "") { $params["AudioFileId"] = $AudioFileId }

New-CsTeamsCallHoldPolicy @params
