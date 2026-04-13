# OMAKURE_SCHEMA_START
# {
#   "Name": "emergency_set_lis_wap",
#   "Description": "Set LIS wireless access point for emergency location",
#   "Tags": ["teams", "emergency", "lis", "configure"],
#   "Fields": [
#     {
#       "Name": "bssid",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "WAP BSSID (e.g. AA-BB-CC-DD-EE-FF)",
#       "Description": "WAP BSSID"
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

$Bssid = ""
$LocationId = ""
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--bssid" { $Bssid = $args[++$i] }
    "--location_id" { $LocationId = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Bssid -eq "") { Write-Error "--bssid is required"; exit 1 }
if ($LocationId -eq "") { Write-Error "--location_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-csonlineliswirelessaccesspoint?view=teams-ps
$params = @{
  BSSID      = $Bssid
  LocationId = $LocationId
}
if ($Description -ne "") { $params["Description"] = $Description }

Set-CsOnlineLisWirelessAccessPoint @params
