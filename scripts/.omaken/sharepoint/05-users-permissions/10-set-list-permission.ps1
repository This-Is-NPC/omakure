#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_list_permission",
#   "Description": "Grant a permission role to a user on a list.",
#   "Fields": [
#     { "Name": "ListName", "Type": "string", "Required": true, "Order": 1, "Arg": "-ListName" },
#     { "Name": "User", "Type": "string", "Required": true, "Order": 2, "Arg": "-User", "Description": "User email" },
#     { "Name": "Role", "Type": "string", "Required": true, "Order": 3, "Arg": "-Role", "Choices": ["Read", "Contribute", "Edit", "Full Control"] }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$ListName,

    [Parameter(Mandatory=$true)]
    [string]$User,

    [Parameter(Mandatory=$true)]
    [ValidateSet("Read", "Contribute", "Edit", "Full Control")]
    [string]$Role
)

Set-PnPListPermission -Identity $ListName -User $User -AddRole $Role
