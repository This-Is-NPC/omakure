# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_upload_app",
#   "Description": "Upload app to catalog",
#   "Tags": ["teams", "apps", "create"],
#   "Fields": [
#     {
#       "Name": "app_path",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Path to .zip app package",
#       "Description": "Path to .zip app package"
#     },
#     {
#       "Name": "distribution_method",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["organization", "sideloaded"],
#       "Default": "organization",
#       "Description": "Distribution method"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$AppPath = ""
$DistributionMethod = "organization"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--app_path" { $AppPath = $args[++$i] }
    "--distribution_method" { $DistributionMethod = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($AppPath -eq "") { Write-Error "--app_path is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-teamsapp?view=teams-ps
$params = @{
  Path = $AppPath
}
if ($DistributionMethod -ne "") { $params["DistributionMethod"] = $DistributionMethod }

New-TeamsApp @params
