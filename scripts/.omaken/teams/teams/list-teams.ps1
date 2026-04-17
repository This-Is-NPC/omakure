# OMAKURE_SCHEMA_START
# {
#   "Name": "teams_list_teams",
#   "Description": "List teams",
#   "Tags": ["teams", "list"],
#   "Fields": [
#     {
#       "Name": "display_name",
#       "Type": "string",
#       "Required": false,
#       "Description": "Filter by display name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$DisplayName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--display_name" { $DisplayName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

# https://learn.microsoft.com/en-us/powershell/module/teams/get-team?view=teams-ps
if ($DisplayName -ne "") {
  Get-Team -DisplayName $DisplayName
} else {
  Get-Team
}
