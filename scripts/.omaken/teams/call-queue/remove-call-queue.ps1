# OMAKURE_SCHEMA_START
# {
#   "Name": "cq_remove",
#   "Description": "Remove call queue",
#   "Tags": ["teams", "call-queue", "remove"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Call Queue ID",
#       "Description": "Call queue identity"
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

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-cscallqueue?view=teams-ps
Remove-CsCallQueue -Identity $Identity
