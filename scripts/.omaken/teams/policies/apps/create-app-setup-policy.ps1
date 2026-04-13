# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_app_setup",
#   "Description": "Create app setup policy",
#   "Tags": ["teams", "policy", "apps", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_user_pinning",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow user pinning"
#     },
#     {
#       "Name": "allow_sideloading",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Allow sideloading"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowUserPinning = "true"
$AllowSideloading = "false"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_user_pinning" { $AllowUserPinning = $args[++$i] }
    "--allow_sideloading" { $AllowSideloading = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsappsetuppolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowUserPinning -ne "") {
  $params["AllowUserPinning"] = if ($AllowUserPinning -eq "true") { $true } else { $false }
}
if ($AllowSideloading -ne "") {
  $params["AllowSideloading"] = if ($AllowSideloading -eq "true") { $true } else { $false }
}

New-CsTeamsAppSetupPolicy @params
