# OMAKURE_SCHEMA_START
# {
#   "Name": "emergency_create_lis_location",
#   "Description": "Create LIS location",
#   "Tags": ["teams", "emergency", "lis", "create"],
#   "Fields": [
#     {
#       "Name": "civic_address_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "Civic address ID"
#     },
#     {
#       "Name": "location",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Location name (e.g. Floor 10)",
#       "Description": "Location name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$CivicAddressId = ""
$Location = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--civic_address_id" { $CivicAddressId = $args[++$i] }
    "--location" { $Location = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($CivicAddressId -eq "") { Write-Error "--civic_address_id is required"; exit 1 }
if ($Location -eq "") { Write-Error "--location is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlinelislocation?view=teams-ps
New-CsOnlineLisLocation -CivicAddressId $CivicAddressId -Location $Location
