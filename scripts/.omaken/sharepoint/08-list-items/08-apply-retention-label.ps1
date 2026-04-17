#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "apply_retention_label_to_item",
#   "Description": "Apply a retention label to a list item.",
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
#     },
#     {
#       "Name": "Label",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-Label",
#       "Prompt": "Retention label name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [int]$ItemId,

    [Parameter(Mandatory = $true)]
    [string]$Label
)

Set-PnPRetentionLabel -List $ListName -Identity $ItemId -Label $Label
