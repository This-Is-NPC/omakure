#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_list_item",
#   "Description": "Add a new item to a list.",
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
#       "Name": "FieldValues",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-FieldValues",
#       "Prompt": "Field values as Key=Value pairs separated by semicolons (e.g. Title=Hello;Status=Active)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

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

Add-PnPListItem -List $ListName -Values $values
