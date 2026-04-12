#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "approve_api_permission",
#   "Description": "Approve a pending tenant service principal API permission request.",
#   "Fields": [
#     {
#       "Name": "RequestId",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-RequestId",
#       "Prompt": "GUID of the pending API permission request to approve"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$RequestId
)

Approve-PnPTenantServicePrincipalPermissionRequest -RequestId $RequestId -Force
Write-Host "API permission request approved: $RequestId"
