# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_events",
#   "Description": "Create events policy",
#   "Tags": ["teams", "policy", "events", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_webinars",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Enabled",
#       "Description": "Allow webinars"
#     },
#     {
#       "Name": "allow_townhalls",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Enabled",
#       "Description": "Allow town halls"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowWebinars = "Enabled"
$AllowTownhalls = "Enabled"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_webinars" { $AllowWebinars = $args[++$i] }
    "--allow_townhalls" { $AllowTownhalls = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamseventspolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowWebinars -ne "") { $params["AllowWebinars"] = $AllowWebinars }
if ($AllowTownhalls -ne "") { $params["AllowTownhalls"] = $AllowTownhalls }

New-CsTeamsEventsPolicy @params
