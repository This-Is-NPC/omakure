# OMAKURE_SCHEMA_START
# {
#   "Name": "aa_import_holidays",
#   "Description": "Import auto attendant holidays",
#   "Tags": ["teams", "auto-attendant", "configure"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Auto Attendant ID",
#       "Description": "Auto attendant identity"
#     },
#     {
#       "Name": "input_file",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "CSV file path",
#       "Description": "Input CSV file path"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$InputFile = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--input_file" { $InputFile = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($InputFile -eq "") { Write-Error "--input_file is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/import-csautoattendantholidays?view=teams-ps
$Bytes = [System.IO.File]::ReadAllBytes($InputFile)
Import-CsAutoAttendantHolidays -Identity $Identity -Input $Bytes
