# OMAKURE_SCHEMA_START
# {
#   "Name": "aa_remove",
#   "Description": "Remove auto attendant",
#   "Tags": ["teams", "auto-attendant", "remove"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Auto Attendant ID",
#       "Description": "Auto attendant identity"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-csautoattendant?view=teams-ps
Remove-CsAutoAttendant -Identity $Identity
