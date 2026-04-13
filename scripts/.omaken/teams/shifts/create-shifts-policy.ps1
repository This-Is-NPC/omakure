# OMAKURE_SCHEMA_START
# {
#   "Name": "shifts_create_policy",
#   "Description": "Create shifts policy",
#   "Tags": ["teams", "shifts", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "enable_schedule_owner_permissions",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "false",
#       "Description": "Enable schedule owner permissions"
#     },
#     {
#       "Name": "access_type",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["UnrestrictedAccess_TeamsApp", "DisabledAccess"],
#       "Default": "UnrestrictedAccess_TeamsApp",
#       "Description": "Access type"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$EnableScheduleOwnerPermissions = "false"
$AccessType = "UnrestrictedAccess_TeamsApp"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--enable_schedule_owner_permissions" { $EnableScheduleOwnerPermissions = $args[++$i] }
    "--access_type" { $AccessType = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsshiftspolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($EnableScheduleOwnerPermissions -ne "") {
  $params["EnableScheduleOwnerPermissions"] = if ($EnableScheduleOwnerPermissions -eq "true") { $true } else { $false }
}
if ($AccessType -ne "") { $params["AccessType"] = $AccessType }

New-CsTeamsShiftsPolicy @params
