#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "remove_field",
#   "Description": "Remove a field from a list or site.",
#   "Fields": [
#     {
#       "Name": "FieldName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-FieldName",
#       "Prompt": "Field internal name"
#     },
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-ListName",
#       "Prompt": "List name (omit for site column)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$FieldName,

    [string]$ListName = ""
)

if ($ListName -ne "") {
    Remove-PnPField -List $ListName -Identity $FieldName -Force
} else {
    Remove-PnPField -Identity $FieldName -Force
}
