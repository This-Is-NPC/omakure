# OMAKURE_SCHEMA_START
# {
#   "Name": "tenant_configure_federation",
#   "Description": "Configure tenant federation settings",
#   "Tags": ["teams", "tenant", "federation", "configure"],
#   "Fields": [
#     {
#       "Name": "allow_federated_users",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow federated users"
#     },
#     {
#       "Name": "allow_teams_consumer",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow Teams consumer access"
#     },
#     {
#       "Name": "allow_teams_consumer_inbound",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow Teams consumer inbound"
#     },
#     {
#       "Name": "blocked_domains",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Blocked domains (comma-separated, leave empty to skip)",
#       "Description": "Blocked domains"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$AllowFederatedUsers = "true"
$AllowTeamsConsumer = "true"
$AllowTeamsConsumerInbound = "true"
$BlockedDomains = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--allow_federated_users" { $AllowFederatedUsers = $args[++$i] }
    "--allow_teams_consumer" { $AllowTeamsConsumer = $args[++$i] }
    "--allow_teams_consumer_inbound" { $AllowTeamsConsumerInbound = $args[++$i] }
    "--blocked_domains" { $BlockedDomains = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

# https://learn.microsoft.com/en-us/powershell/module/teams/set-cstenantfederationconfiguration?view=teams-ps
$params = @{}
if ($AllowFederatedUsers -ne "") {
  $params["AllowFederatedUsers"] = if ($AllowFederatedUsers -eq "true") { $true } else { $false }
}
if ($AllowTeamsConsumer -ne "") {
  $params["AllowTeamsConsumer"] = if ($AllowTeamsConsumer -eq "true") { $true } else { $false }
}
if ($AllowTeamsConsumerInbound -ne "") {
  $params["AllowTeamsConsumerInbound"] = if ($AllowTeamsConsumerInbound -eq "true") { $true } else { $false }
}
if ($BlockedDomains -ne "") {
  $DomainArray = $BlockedDomains -split ","
  $DomainList = @()
  foreach ($d in $DomainArray) {
    $DomainList += New-CsEdgeDomainPattern -Domain $d.Trim()
  }
  $params["BlockedDomains"] = $DomainList
}

Set-CsTenantFederationConfiguration @params
