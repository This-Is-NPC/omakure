#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "delete_list_item",
#   "Description": "Delete a list item (moves to recycle bin).",
#   "Fields": [
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ListName",
#       "Prompt": "List name"
#     },
#     {
#       "Name": "ItemId",
#       "Type": "number",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-ItemId",
#       "Prompt": "Item ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [int]$ItemId
)

Remove-PnPListItem -List $ListName -Identity $ItemId -Recycle -Force
