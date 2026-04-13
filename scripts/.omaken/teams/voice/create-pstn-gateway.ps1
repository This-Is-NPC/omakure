# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_create_pstn_gateway",
#   "Description": "Create PSTN gateway (SBC)",
#   "Tags": ["teams", "voice", "direct-routing", "sbc", "create"],
#   "Fields": [
#     {
#       "Name": "fqdn",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "SBC FQDN",
#       "Description": "SBC fully qualified domain name"
#     },
#     {
#       "Name": "sip_signaling_port",
#       "Type": "string",
#       "Required": true,
#       "Default": "5067",
#       "Description": "SIP signaling port"
#     },
#     {
#       "Name": "max_concurrent_sessions",
#       "Type": "string",
#       "Required": false,
#       "Default": "100",
#       "Description": "Maximum concurrent sessions"
#     },
#     {
#       "Name": "enabled",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Enable the gateway"
#     },
#     {
#       "Name": "media_bypass",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Enable media bypass"
#     },
#     {
#       "Name": "gateway_site_id",
#       "Type": "string",
#       "Required": false,
#       "Description": "Gateway site ID"
#     },
#     {
#       "Name": "send_sip_options",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Send SIP options"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Fqdn = ""
$SipSignalingPort = "5067"
$MaxConcurrentSessions = "100"
$Enabled = "true"
$MediaBypass = "false"
$GatewaySiteId = ""
$SendSipOptions = "true"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--fqdn" { $Fqdn = $args[++$i] }
    "--sip_signaling_port" { $SipSignalingPort = $args[++$i] }
    "--max_concurrent_sessions" { $MaxConcurrentSessions = $args[++$i] }
    "--enabled" { $Enabled = $args[++$i] }
    "--media_bypass" { $MediaBypass = $args[++$i] }
    "--gateway_site_id" { $GatewaySiteId = $args[++$i] }
    "--send_sip_options" { $SendSipOptions = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Fqdn -eq "") { Write-Error "--fqdn is required"; exit 1 }
if ($SipSignalingPort -eq "") { Write-Error "--sip_signaling_port is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlinepstngateway?view=teams-ps
$params = @{
  Fqdn             = $Fqdn
  SipSignalingPort = [int]$SipSignalingPort
}
if ($MaxConcurrentSessions -ne "") { $params["MaxConcurrentSessions"] = [int]$MaxConcurrentSessions }
if ($Enabled -ne "") {
  $params["Enabled"] = if ($Enabled -eq "true") { $true } else { $false }
}
if ($MediaBypass -ne "") {
  $params["MediaBypass"] = if ($MediaBypass -eq "true") { $true } else { $false }
}
if ($GatewaySiteId -ne "") { $params["GatewaySiteId"] = $GatewaySiteId }
if ($SendSipOptions -ne "") {
  $params["SendSipOptions"] = if ($SendSipOptions -eq "true") { $true } else { $false }
}

New-CsOnlinePSTNGateway @params
