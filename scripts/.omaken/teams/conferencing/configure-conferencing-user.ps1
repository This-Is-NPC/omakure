# OMAKURE_SCHEMA_START
# {
#   "Name": "conf_configure_user",
#   "Description": "Configure dial-in conferencing user",
#   "Tags": ["teams", "conferencing", "configure"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email",
#       "Description": "User identity"
#     },
#     {
#       "Name": "service_number",
#       "Type": "string",
#       "Required": false,
#       "Description": "Service number"
#     },
#     {
#       "Name": "allow_toll_free",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow toll-free dial-in"
#     },
#     {
#       "Name": "reset_leader_pin",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Reset leader PIN"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$ServiceNumber = ""
$AllowTollFree = "true"
$ResetLeaderPin = "false"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--service_number" { $ServiceNumber = $args[++$i] }
    "--allow_toll_free" { $AllowTollFree = $args[++$i] }
    "--reset_leader_pin" { $ResetLeaderPin = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-csonlinedialinconferencinguser?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($ServiceNumber -ne "") { $params["ServiceNumber"] = $ServiceNumber }
if ($AllowTollFree -ne "") {
  $params["AllowTollFreeDialIn"] = if ($AllowTollFree -eq "true") { $true } else { $false }
}
if ($ResetLeaderPin -ne "") {
  $params["ResetLeaderPIN"] = if ($ResetLeaderPin -eq "true") { $true } else { $false }
}

Set-CsOnlineDialInConferencingUser @params
