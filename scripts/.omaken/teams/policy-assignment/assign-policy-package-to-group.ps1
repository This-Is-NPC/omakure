# OMAKURE_SCHEMA_START
# {
#   "Name": "assignment_package_to_group",
#   "Description": "Assign policy package to group",
#   "Tags": ["teams", "policy", "package", "group", "grant"],
#   "Fields": [
#     {
#       "Name": "group_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "Group ID"
#     },
#     {
#       "Name": "package_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy package name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$GroupId = ""
$PackageName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--group_id" { $GroupId = $args[++$i] }
    "--package_name" { $PackageName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($GroupId -eq "") { Write-Error "--group_id is required"; exit 1 }
if ($PackageName -eq "") { Write-Error "--package_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/grant-csgrouppolicypackageassignment?view=teams-ps
Grant-CsGroupPolicyPackageAssignment -GroupId $GroupId -PackageName $PackageName
