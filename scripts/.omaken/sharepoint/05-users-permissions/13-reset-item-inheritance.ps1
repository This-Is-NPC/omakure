#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "reset_item_inheritance",
#   "Description": "Reset an item to inherit permissions from the list.",
#   "Fields": [
#     { "Name": "ListName", "Type": "string", "Required": true, "Order": 1, "Arg": "-ListName" },
#     { "Name": "ItemId", "Type": "number", "Required": true, "Order": 2, "Arg": "-ItemId" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$ListName,

    [Parameter(Mandatory=$true)]
    [int]$ItemId
)

Set-PnPListItemPermission -List $ListName -Identity $ItemId -InheritPermissions
