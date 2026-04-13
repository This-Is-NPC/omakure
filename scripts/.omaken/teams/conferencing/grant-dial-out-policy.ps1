# OMAKURE_SCHEMA_START
# {
#   "Name": "conf_grant_dial_out",
#   "Description": "Grant dial-out policy to user",
#   "Tags": ["teams", "conferencing", "policy", "grant"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email",
#       "Description": "User identity"
#     },
#     {
#       "Name": "policy_name",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["DialoutCPCandPSTNInternational", "DialoutCPCDomesticPSTNInternational", "DialoutCPCDisabledPSTNInternational", "DialoutCPCandPSTNDomestic", "DialoutCPCInternationalPSTNDomestic", "DialoutCPCDomesticPSTNDisabled", "DialoutCPCDisabledPSTNDomestic", "DialoutCPCandPSTNDisabled", "DialoutCPCDomesticPSTNInternational"],
#       "Description": "Dial-out policy name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$PolicyName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--policy_name" { $PolicyName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($PolicyName -eq "") { Write-Error "--policy_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/grant-csdialoutpolicy?view=teams-ps
Grant-CsDialoutPolicy -Identity $Identity -PolicyName $PolicyName
