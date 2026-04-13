#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_custom_role",
#   "Description": "Create a custom permission level by cloning an existing one.",
#   "Fields": [
#     { "Name": "RoleName", "Type": "string", "Required": true, "Order": 1, "Arg": "-RoleName", "Description": "New role name" },
#     { "Name": "CloneFrom", "Type": "string", "Required": false, "Order": 2, "Arg": "-CloneFrom", "Description": "Existing role to clone (e.g. Contribute)", "Default": "Contribute" },
#     { "Name": "Description", "Type": "string", "Required": false, "Order": 3, "Arg": "-Description" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$RoleName,

    [string]$CloneFrom = "Contribute",

    [string]$Description = ""
)

$params = @{
    RoleName = $RoleName
    Clone    = $CloneFrom
}

if ($Description) {
    $params["Description"] = $Description
}

Add-PnPRoleDefinition @params
