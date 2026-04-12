# OMAKURE_SCHEMA_START
# {
#   "Name": "network_create_subnet",
#   "Description": "Create network subnet",
#   "Tags": ["teams", "network", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Subnet (e.g. 10.10.10.0)",
#       "Description": "Subnet address"
#     },
#     {
#       "Name": "mask_bits",
#       "Type": "string",
#       "Required": true,
#       "Default": "24",
#       "Description": "Subnet mask bits"
#     },
#     {
#       "Name": "network_site_id",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Site Name",
#       "Description": "Network site ID"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$MaskBits = "24"
$NetworkSiteId = ""
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--mask_bits" { $MaskBits = $args[++$i] }
    "--network_site_id" { $NetworkSiteId = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($NetworkSiteId -eq "") { Write-Error "--network_site_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-cstenantnetworksubnet?view=teams-ps
$params = @{
  Identity      = $Identity
  MaskBits      = [int]$MaskBits
  NetworkSiteID = $NetworkSiteId
}
if ($Description -ne "") { $params["Description"] = $Description }

New-CsTenantNetworkSubnet @params
