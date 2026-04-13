# OMAKURE_SCHEMA_START
# {
#   "Name": "network_create_region",
#   "Description": "Create network region",
#   "Tags": ["teams", "network", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Region Name",
#       "Description": "Network region name"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description"
#     },
#     {
#       "Name": "central_site",
#       "Type": "string",
#       "Required": false,
#       "Description": "Central site"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Description = ""
$CentralSite = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--central_site" { $CentralSite = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-cstenantnetworkregion?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($Description -ne "") { $params["Description"] = $Description }
if ($CentralSite -ne "") { $params["CentralSite"] = $CentralSite }

New-CsTenantNetworkRegion @params
