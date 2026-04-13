# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_remove_calling",
#   "Description": "Remove calling policy",
#   "Tags": ["teams", "policy", "calling", "remove"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Policy Name",
#       "Description": "Policy name"
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

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-csteamscallingpolicy?view=teams-ps
Remove-CsTeamsCallingPolicy -Identity $Identity
