#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "update_list_item",
#   "Description": "Update an existing list item.",
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
#       "Name": "FieldValues",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-FieldValues",
#       "Prompt": "Field values as Key=Value pairs separated by semicolons"
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
    [string]$FieldValues
)

$values = @{}
foreach ($pair in $FieldValues.Split(";")) {
    $parts = $pair.Split("=", 2)
    if ($parts.Count -eq 2) {
        $values[$parts[0].Trim()] = $parts[1].Trim()
    }
}

Set-PnPListItem -List $ListName -Identity $ItemId -Values $values
