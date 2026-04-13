# OMAKURE_SCHEMA_START
# {
#   "Name": "compliance_create_policy",
#   "Description": "Create compliance recording policy",
#   "Tags": ["teams", "compliance", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "enabled",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Enable policy"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description"
#     },
#     {
#       "Name": "warn_user_on_removal",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Warn user on removal"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Enabled = "true"
$Description = ""
$WarnUserOnRemoval = "true"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--enabled" { $Enabled = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--warn_user_on_removal" { $WarnUserOnRemoval = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamscompliancerecordingpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($Enabled -ne "") {
  $params["Enabled"] = if ($Enabled -eq "true") { $true } else { $false }
}
if ($Description -ne "") { $params["Description"] = $Description }
if ($WarnUserOnRemoval -ne "") {
  $params["WarnUserOnRemoval"] = if ($WarnUserOnRemoval -eq "true") { $true } else { $false }
}

New-CsTeamsComplianceRecordingPolicy @params
