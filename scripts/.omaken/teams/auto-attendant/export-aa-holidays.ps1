# OMAKURE_SCHEMA_START
# {
#   "Name": "aa_export_holidays",
#   "Description": "Export auto attendant holidays",
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
#       "Name": "output_file",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Output file path",
#       "Description": "Output file path"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$OutputFile = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--output_file" { $OutputFile = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($OutputFile -eq "") { Write-Error "--output_file is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/export-csautoattendantholidays?view=teams-ps
$Bytes = Export-CsAutoAttendantHolidays -Identity $Identity
[System.IO.File]::WriteAllBytes($OutputFile, $Bytes)
