# OMAKURE_SCHEMA_START
# {
#   "Name": "assignment_create_batch",
#   "Description": "Create batch policy assignment",
#   "Tags": ["teams", "policy", "batch", "create"],
#   "Fields": [
#     {
#       "Name": "policy_type",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["TeamsCallingPolicy", "TeamsMessagingPolicy", "TeamsMeetingPolicy", "TeamsAppSetupPolicy", "TeamsAppPermissionPolicy"],
#       "Description": "Policy type"
#     },
#     {
#       "Name": "policy_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "users",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User emails (comma-separated)",
#       "Description": "User emails comma-separated"
#     },
#     {
#       "Name": "operation_name",
#       "Type": "string",
#       "Required": false,
#       "Description": "Operation name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$PolicyType = ""
$PolicyName = ""
$Users = ""
$OperationName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--policy_type" { $PolicyType = $args[++$i] }
    "--policy_name" { $PolicyName = $args[++$i] }
    "--users" { $Users = $args[++$i] }
    "--operation_name" { $OperationName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($PolicyType -eq "") { Write-Error "--policy_type is required"; exit 1 }
if ($PolicyName -eq "") { Write-Error "--policy_name is required"; exit 1 }
if ($Users -eq "") { Write-Error "--users is required"; exit 1 }

$UserArray = $Users -split ","

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csbatchpolicyassignmentoperation?view=teams-ps
$params = @{
  PolicyType = $PolicyType
  PolicyName = $PolicyName
  Identity   = @($UserArray)
}
if ($OperationName -ne "") { $params["OperationName"] = $OperationName }

New-CsBatchPolicyAssignmentOperation @params
