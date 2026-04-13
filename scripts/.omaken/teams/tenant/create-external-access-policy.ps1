# OMAKURE_SCHEMA_START
# {
#   "Name": "tenant_create_external_access",
#   "Description": "Create external access policy",
#   "Tags": ["teams", "tenant", "federation", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "enable_federation_access",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Enable federation access"
#     },
#     {
#       "Name": "enable_teams_consumer_access",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Enable Teams consumer access"
#     },
#     {
#       "Name": "enable_teams_consumer_inbound",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Enable Teams consumer inbound"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$EnableFederationAccess = "true"
$EnableTeamsConsumerAccess = "false"
$EnableTeamsConsumerInbound = "false"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--enable_federation_access" { $EnableFederationAccess = $args[++$i] }
    "--enable_teams_consumer_access" { $EnableTeamsConsumerAccess = $args[++$i] }
    "--enable_teams_consumer_inbound" { $EnableTeamsConsumerInbound = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csexternalaccesspolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($EnableFederationAccess -ne "") {
  $params["EnableFederationAccess"] = if ($EnableFederationAccess -eq "true") { $true } else { $false }
}
if ($EnableTeamsConsumerAccess -ne "") {
  $params["EnableTeamsConsumerAccess"] = if ($EnableTeamsConsumerAccess -eq "true") { $true } else { $false }
}
if ($EnableTeamsConsumerInbound -ne "") {
  $params["EnableTeamsConsumerInbound"] = if ($EnableTeamsConsumerInbound -eq "true") { $true } else { $false }
}

New-CsExternalAccessPolicy @params
