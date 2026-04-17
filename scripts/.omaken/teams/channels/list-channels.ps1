# OMAKURE_SCHEMA_START
# {
#   "Name": "channels_list_channels",
#   "Description": "List channels",
#   "Tags": ["teams", "channels", "list"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "The group ID of the team"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/get-teamchannel?view=teams-ps
Get-TeamChannel -GroupId $GroupId
