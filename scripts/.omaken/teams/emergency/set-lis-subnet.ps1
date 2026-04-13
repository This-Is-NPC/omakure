# OMAKURE_SCHEMA_START
# {
#   "Name": "emergency_set_lis_subnet",
#   "Description": "Set LIS subnet for emergency location",
#   "Tags": ["teams", "emergency", "lis", "configure"],
#   "Fields": [
#     {
#       "Name": "subnet",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Subnet (e.g. 10.10.10.0)",
#       "Description": "Subnet address"
#     },
#     {
#       "Name": "location_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "Location ID"
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

$Subnet = ""
$LocationId = ""
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--subnet" { $Subnet = $args[++$i] }
    "--location_id" { $LocationId = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Subnet -eq "") { Write-Error "--subnet is required"; exit 1 }
if ($LocationId -eq "") { Write-Error "--location_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-csonlinelissubnet?view=teams-ps
$params = @{
  Subnet     = $Subnet
  LocationId = $LocationId
}
if ($Description -ne "") { $params["Description"] = $Description }

Set-CsOnlineLisSubnet @params
