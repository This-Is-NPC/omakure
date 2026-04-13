# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_archive_team",
#   "Description": "Archive or unarchive a team",
#   "Tags": ["teams", "configure"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
#     },
#     {
#       "Name": "archived",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["true", "false"],
#       "Description": "Whether to archive or unarchive the team"
#     },
#     {
#       "Name": "set_spo_readonly",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Set SharePoint site as read-only"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$Archived = ""
$SetSpoReadonly = "true"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--archived" { $Archived = $args[++$i] }
    "--set_spo_readonly" { $SetSpoReadonly = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($Archived -eq "") { Write-Error "--archived is required"; exit 1 }

# Convert string booleans to PowerShell booleans
$ArchivedBool = if ($Archived -eq "true") { $true } else { $false }
$SetSpoReadonlyBool = if ($SetSpoReadonly -eq "true") { $true } else { $false }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-teamarchivedstatus?view=teams-ps
Set-TeamArchivedState -GroupId $GroupId -Archived $ArchivedBool -SetSpoSiteReadOnlyForMembers $SetSpoReadonlyBool
