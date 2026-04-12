# OMAKURE_SCHEMA_START
# {
#   "Name": "users_add_calling_delegate",
#   "Description": "Add call delegate",
#   "Tags": ["teams", "users", "voice", "create"],
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
#     },
#     {
#       "Name": "make_calls",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow delegate to make calls"
#     },
#     {
#       "Name": "receive_calls",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow delegate to receive calls"
#     },
#     {
#       "Name": "manage_settings",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Allow delegate to manage call settings"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Delegate = ""
$MakeCalls = "true"
$ReceiveCalls = "true"
$ManageSettings = "false"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--delegate" { $Delegate = $args[++$i] }
    "--make_calls" { $MakeCalls = $args[++$i] }
    "--receive_calls" { $ReceiveCalls = $args[++$i] }
    "--manage_settings" { $ManageSettings = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($Delegate -eq "") { Write-Error "--delegate is required"; exit 1 }

$MakeCallsBool = if ($MakeCalls -eq "true") { $true } else { $false }
$ReceiveCallsBool = if ($ReceiveCalls -eq "true") { $true } else { $false }
$ManageSettingsBool = if ($ManageSettings -eq "true") { $true } else { $false }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csusercallingdelegate?view=teams-ps
New-CsUserCallingDelegate -Identity $Identity -Delegate $Delegate -MakeCalls $MakeCallsBool -ReceiveCalls $ReceiveCallsBool -ManageSettings $ManageSettingsBool
