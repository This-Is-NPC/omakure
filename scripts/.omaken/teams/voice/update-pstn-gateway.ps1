# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_update_pstn_gateway",
#   "Description": "Update PSTN gateway (SBC) settings",
#   "Tags": ["teams", "voice", "direct-routing", "sbc", "configure"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "SBC FQDN",
#       "Description": "SBC fully qualified domain name"
#     },
#     {
#       "Name": "max_concurrent_sessions",
#       "Type": "string",
#       "Required": false,
#       "Description": "Maximum concurrent sessions"
#     },
#     {
#       "Name": "media_bypass",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Description": "Enable media bypass"
#     },
#     {
#       "Name": "enabled",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Description": "Enable the gateway"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$MaxConcurrentSessions = ""
$MediaBypass = ""
$Enabled = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--max_concurrent_sessions" { $MaxConcurrentSessions = $args[++$i] }
    "--media_bypass" { $MediaBypass = $args[++$i] }
    "--enabled" { $Enabled = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-csonlinepstngateway?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($MaxConcurrentSessions -ne "") { $params["MaxConcurrentSessions"] = [int]$MaxConcurrentSessions }
if ($MediaBypass -ne "") {
  $params["MediaBypass"] = if ($MediaBypass -eq "true") { $true } else { $false }
}
if ($Enabled -ne "") {
  $params["Enabled"] = if ($Enabled -eq "true") { $true } else { $false }
}

Set-CsOnlinePSTNGateway @params
