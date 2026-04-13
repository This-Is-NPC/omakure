# OMAKURE_SCHEMA_START
# {
#   "Name": "assignment_check_batch_status",
#   "Description": "Check batch policy assignment status",
#   "Tags": ["teams", "policy", "batch", "list"],
#   "Fields": [
#     {
#       "Name": "operation_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "Operation ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$OperationId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--operation_id" { $OperationId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($OperationId -eq "") { Write-Error "--operation_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/get-csbatchpolicyassignmentoperation?view=teams-ps
Get-CsBatchPolicyAssignmentOperation -OperationId $OperationId
