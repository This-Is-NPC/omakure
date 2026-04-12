#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "apply_retention_label",
#   "Description": "Apply a retention label to a SharePoint list or library.",
#   "Fields": [
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ListName",
#       "Prompt": "Name of the list or library"
#     },
#     {
#       "Name": "Label",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Label",
#       "Prompt": "Name of the retention label to apply"
#     },
#     {
#       "Name": "SyncToItems",
#       "Type": "bool",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-SyncToItems",
#       "Default": "false",
#       "Prompt": "Sync the label to existing items in the list"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [string]$Label,

    [Parameter(Mandatory = $false)]
    [bool]$SyncToItems = $false
)

Set-PnPLabel -List $ListName -Label $Label -SyncToItems $SyncToItems
Write-Host "Retention label '$Label' applied to list '$ListName' (SyncToItems: $SyncToItems)."
