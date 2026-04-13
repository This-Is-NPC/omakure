# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_app_permission",
#   "Description": "Create app permission policy",
#   "Tags": ["teams", "policy", "apps", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "default_catalog_apps_type",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["AllowedAppList", "BlockedAppList"],
#       "Default": "AllowedAppList",
#       "Description": "Default catalog apps type"
#     },
#     {
#       "Name": "global_catalog_apps_type",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["AllowedAppList", "BlockedAppList"],
#       "Default": "AllowedAppList",
#       "Description": "Global catalog apps type"
#     },
#     {
#       "Name": "private_catalog_apps_type",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["AllowedAppList", "BlockedAppList"],
#       "Default": "AllowedAppList",
#       "Description": "Private catalog apps type"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$DefaultCatalogAppsType = "AllowedAppList"
$GlobalCatalogAppsType = "AllowedAppList"
$PrivateCatalogAppsType = "AllowedAppList"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--default_catalog_apps_type" { $DefaultCatalogAppsType = $args[++$i] }
    "--global_catalog_apps_type" { $GlobalCatalogAppsType = $args[++$i] }
    "--private_catalog_apps_type" { $PrivateCatalogAppsType = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsapppermissionpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($DefaultCatalogAppsType -ne "") { $params["DefaultCatalogAppsType"] = $DefaultCatalogAppsType }
if ($GlobalCatalogAppsType -ne "") { $params["GlobalCatalogAppsType"] = $GlobalCatalogAppsType }
if ($PrivateCatalogAppsType -ne "") { $params["PrivateCatalogAppsType"] = $PrivateCatalogAppsType }

New-CsTeamsAppPermissionPolicy @params
