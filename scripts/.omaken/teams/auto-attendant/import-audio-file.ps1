# OMAKURE_SCHEMA_START
# {
#   "Name": "aa_import_audio",
#   "Description": "Import audio file for auto attendant or call queue",
#   "Tags": ["teams", "auto-attendant", "call-queue", "configure"],
#   "Fields": [
#     {
#       "Name": "application_id",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["OrgAutoAttendant", "HuntGroup"],
#       "Default": "OrgAutoAttendant",
#       "Description": "Application type"
#     },
#     {
#       "Name": "file_name",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Audio file name",
#       "Description": "Audio file name"
#     },
#     {
#       "Name": "file_path",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Path to .wav file",
#       "Description": "Path to wav file"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$ApplicationId = "OrgAutoAttendant"
$FileName = ""
$FilePath = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--application_id" { $ApplicationId = $args[++$i] }
    "--file_name" { $FileName = $args[++$i] }
    "--file_path" { $FilePath = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($FileName -eq "") { Write-Error "--file_name is required"; exit 1 }
if ($FilePath -eq "") { Write-Error "--file_path is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/import-csonlineaudiofile?view=teams-ps
$FileBytes = [System.IO.File]::ReadAllBytes($FilePath)
Import-CsOnlineAudioFile -ApplicationId $ApplicationId -FileName $FileName -Content $FileBytes
