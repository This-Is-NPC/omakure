# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_encryption",
#   "Description": "Create encryption policy",
#   "Tags": ["teams", "policy", "encryption", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "calling_e2ee",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Disabled", "DisabledUserOverride", "Enabled"],
#       "Default": "DisabledUserOverride",
#       "Description": "Calling end-to-end encryption enabled type"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$CallingE2EE = "DisabledUserOverride"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--calling_e2ee" { $CallingE2EE = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsenhancedencryptionpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($CallingE2EE -ne "") { $params["CallingEndToEndEncryptionEnabledType"] = $CallingE2EE }

New-CsTeamsEnhancedEncryptionPolicy @params
