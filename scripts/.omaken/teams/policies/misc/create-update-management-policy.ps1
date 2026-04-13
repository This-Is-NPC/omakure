# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_update_mgmt",
#   "Description": "Create update management policy",
#   "Tags": ["teams", "policy", "update", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_managed_updates",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow managed updates"
#     },
#     {
#       "Name": "allow_preview",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Allow preview"
#     },
#     {
#       "Name": "update_day",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"],
#       "Default": "Saturday",
#       "Description": "Update day of week"
#     },
#     {
#       "Name": "update_time",
#       "Type": "string",
#       "Required": false,
#       "Default": "03:00",
#       "Description": "Update time"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowManagedUpdates = "true"
$AllowPreview = "false"
$UpdateDay = "Saturday"
$UpdateTime = "03:00"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_managed_updates" { $AllowManagedUpdates = $args[++$i] }
    "--allow_preview" { $AllowPreview = $args[++$i] }
    "--update_day" { $UpdateDay = $args[++$i] }
    "--update_time" { $UpdateTime = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsupdatemanagementpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowManagedUpdates -ne "") {
  $params["AllowManagedUpdates"] = if ($AllowManagedUpdates -eq "true") { $true } else { $false }
}

if ($AllowManagedUpdates -eq "true") {
  $dayMap = @{
    Sunday = 0
    Monday = 1
    Tuesday = 2
    Wednesday = 3
    Thursday = 4
    Friday = 5
    Saturday = 6
  }

  if ($AllowPreview -ne "") {
    $params["AllowPreview"] = if ($AllowPreview -eq "true") { $true } else { $false }
  }
  if ($UpdateDay -ne "") { $params["UpdateDayOfWeek"] = $dayMap[$UpdateDay] }
  if ($UpdateTime -ne "") { $params["UpdateTime"] = $UpdateTime }
}

New-CsTeamsUpdateManagementPolicy @params
