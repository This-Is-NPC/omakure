#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_item_permission",
#   "Description": "Grant a permission role to a user on a list item.",
#   "Fields": [
#     { "Name": "ListName", "Type": "string", "Required": true, "Order": 1, "Arg": "-ListName" },
#     { "Name": "ItemId", "Type": "number", "Required": true, "Order": 2, "Arg": "-ItemId" },
#     { "Name": "User", "Type": "string", "Required": true, "Order": 3, "Arg": "-User", "Description": "User email" },
#     { "Name": "Role", "Type": "string", "Required": true, "Order": 4, "Arg": "-Role", "Choices": ["Read", "Contribute", "Edit", "Full Control"] }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$ListName,

    [Parameter(Mandatory=$true)]
    [int]$ItemId,

    [Parameter(Mandatory=$true)]
    [string]$User,

    [Parameter(Mandatory=$true)]
    [ValidateSet("Read", "Contribute", "Edit", "Full Control")]
    [string]$Role
)

Set-PnPListItemPermission -List $ListName -Identity $ItemId -User $User -AddRole $Role
