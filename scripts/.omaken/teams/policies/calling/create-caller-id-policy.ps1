# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_caller_id",
#   "Description": "Create caller ID policy",
#   "Tags": ["teams", "policy", "calling", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "calling_id_substitute",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["LineUri", "Anonymous", "Service", "Resource"],
#       "Description": "Calling ID substitute type"
#     },
#     {
#       "Name": "service_number",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Service number (if substitute=Service)",
#       "Description": "Service number"
#     },
#     {
#       "Name": "resource_account",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Resource account ID (if substitute=Resource)",
#       "Description": "Resource account ID"
#     },
#     {
#       "Name": "enable_user_override",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Enable user override"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$CallingIdSubstitute = ""
$ServiceNumber = ""
$ResourceAccount = ""
$EnableUserOverride = "false"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--calling_id_substitute" { $CallingIdSubstitute = $args[++$i] }
    "--service_number" { $ServiceNumber = $args[++$i] }
    "--resource_account" { $ResourceAccount = $args[++$i] }
    "--enable_user_override" { $EnableUserOverride = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($CallingIdSubstitute -eq "") { Write-Error "--calling_id_substitute is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-cscallinglineidentity?view=teams-ps
$params = @{
  Identity             = $Identity
  CallingIDSubstitute  = $CallingIdSubstitute
}
if ($ServiceNumber -ne "") { $params["ServiceNumber"] = $ServiceNumber }
if ($ResourceAccount -ne "") { $params["ResourceAccount"] = $ResourceAccount }
if ($EnableUserOverride -ne "") {
  $params["EnableUserOverride"] = if ($EnableUserOverride -eq "true") { $true } else { $false }
}

New-CsCallingLineIdentity @params
