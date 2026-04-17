# OMAKURE_SCHEMA_START
# {
#   "Name": "users_get_policy_assignments",
#   "Description": "List user policy assignments",
#   "Tags": ["teams", "users", "policy", "list"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email or UPN",
#       "Description": "User email or UPN"
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

# https://learn.microsoft.com/en-us/powershell/module/teams/get-csuserpolicyassignment?view=teams-ps
Get-CsUserPolicyAssignment -Identity $Identity
