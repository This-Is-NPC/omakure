# OMAKURE_SCHEMA_START
# {
#   "Name": "users_remove_calling_delegate",
#   "Description": "Remove delegate",
#   "Tags": ["teams", "users", "voice", "remove"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email or UPN",
#       "Description": "User email or UPN"
#     },
#     {
#       "Name": "delegate",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Delegate email",
#       "Description": "Delegate email address"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Delegate = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--delegate" { $Delegate = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($Delegate -eq "") { Write-Error "--delegate is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-csusercallingdelegate?view=teams-ps
Remove-CsUserCallingDelegate -Identity $Identity -Delegate $Delegate
