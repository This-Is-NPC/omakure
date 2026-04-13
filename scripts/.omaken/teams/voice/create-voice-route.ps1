# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_create_voice_route",
#   "Description": "Create online voice route",
#   "Tags": ["teams", "voice", "direct-routing", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Route Name",
#       "Description": "Voice route name"
#     },
#     {
#       "Name": "number_pattern",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Regex pattern (e.g. ^\\+1\\d{10}$)",
#       "Description": "Number pattern regex"
#     },
#     {
#       "Name": "gateway_list",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "SBC FQDN (comma-separated for multiple)",
#       "Description": "SBC FQDNs comma-separated"
#     },
#     {
#       "Name": "priority",
#       "Type": "string",
#       "Required": false,
#       "Default": "1",
#       "Description": "Route priority"
#     },
#     {
#       "Name": "online_pstn_usages",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "PSTN usage name",
#       "Description": "PSTN usage name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$NumberPattern = ""
$GatewayList = ""
$Priority = "1"
$OnlinePstnUsages = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--number_pattern" { $NumberPattern = $args[++$i] }
    "--gateway_list" { $GatewayList = $args[++$i] }
    "--priority" { $Priority = $args[++$i] }
    "--online_pstn_usages" { $OnlinePstnUsages = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($NumberPattern -eq "") { Write-Error "--number_pattern is required"; exit 1 }
if ($GatewayList -eq "") { Write-Error "--gateway_list is required"; exit 1 }
if ($OnlinePstnUsages -eq "") { Write-Error "--online_pstn_usages is required"; exit 1 }

$GatewayArray = $GatewayList -split ","

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlinevoiceroute?view=teams-ps
$params = @{
  Identity                = $Identity
  NumberPattern           = $NumberPattern
  OnlinePstnGatewayList   = @($GatewayArray)
  OnlinePstnUsages        = @($OnlinePstnUsages)
}
if ($Priority -ne "") { $params["Priority"] = [int]$Priority }

New-CsOnlineVoiceRoute @params
