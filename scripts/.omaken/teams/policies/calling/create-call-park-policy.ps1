# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_call_park",
#   "Description": "Create call park policy",
#   "Tags": ["teams", "policy", "calling", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_call_park",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow call park"
#     },
#     {
#       "Name": "pickup_range_start",
#       "Type": "string",
#       "Required": false,
#       "Default": "10",
#       "Description": "Pickup range start"
#     },
#     {
#       "Name": "pickup_range_end",
#       "Type": "string",
#       "Required": false,
#       "Default": "99",
#       "Description": "Pickup range end"
#     },
#     {
#       "Name": "park_timeout_seconds",
#       "Type": "string",
#       "Required": false,
#       "Default": "300",
#       "Description": "Park timeout in seconds"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowCallPark = "true"
$PickupRangeStart = "10"
$PickupRangeEnd = "99"
$ParkTimeoutSeconds = "300"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_call_park" { $AllowCallPark = $args[++$i] }
    "--pickup_range_start" { $PickupRangeStart = $args[++$i] }
    "--pickup_range_end" { $PickupRangeEnd = $args[++$i] }
    "--park_timeout_seconds" { $ParkTimeoutSeconds = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamscallparkpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowCallPark -ne "") {
  $params["AllowCallPark"] = if ($AllowCallPark -eq "true") { $true } else { $false }
}
if ($PickupRangeStart -ne "") { $params["PickupRangeStart"] = [int]$PickupRangeStart }
if ($PickupRangeEnd -ne "") { $params["PickupRangeEnd"] = [int]$PickupRangeEnd }
if ($ParkTimeoutSeconds -ne "") { $params["ParkTimeoutSeconds"] = [int]$ParkTimeoutSeconds }

New-CsTeamsCallParkPolicy @params
