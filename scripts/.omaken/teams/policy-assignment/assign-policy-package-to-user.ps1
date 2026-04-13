# OMAKURE_SCHEMA_START
# {
#   "Name": "assignment_package_to_user",
#   "Description": "Assign policy package to user",
#   "Tags": ["teams", "policy", "package", "grant"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email",
#       "Description": "User identity"
#     },
#     {
#       "Name": "package_name",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Package name (e.g. Education_Teacher, Frontline_Worker)",
#       "Description": "Policy package name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$PackageName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--package_name" { $PackageName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($PackageName -eq "") { Write-Error "--package_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/grant-csuserpolicypackage?view=teams-ps
Grant-CsUserPolicyPackage -Identity $Identity -PackageName $PackageName
