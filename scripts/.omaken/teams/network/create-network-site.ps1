# OMAKURE_SCHEMA_START
# {
#   "Name": "network_create_site",
#   "Description": "Create network site",
#   "Tags": ["teams", "network", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Site Name",
#       "Description": "Network site name"
#     },
#     {
#       "Name": "network_region_id",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Region Name",
#       "Description": "Network region ID"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description"
#     },
#     {
#       "Name": "enable_location_based_routing",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Enable location-based routing"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$NetworkRegionId = ""
$Description = ""
$EnableLocationBasedRouting = "false"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--network_region_id" { $NetworkRegionId = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--enable_location_based_routing" { $EnableLocationBasedRouting = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($NetworkRegionId -eq "") { Write-Error "--network_region_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-cstenantnetworksite?view=teams-ps
$params = @{
  Identity        = $Identity
  NetworkRegionID = $NetworkRegionId
}
if ($Description -ne "") { $params["Description"] = $Description }
if ($EnableLocationBasedRouting -ne "") {
  $params["EnableLocationBasedRouting"] = if ($EnableLocationBasedRouting -eq "true") { $true } else { $false }
}

New-CsTenantNetworkSite @params
