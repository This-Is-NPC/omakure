#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "export_items_to_csv",
#   "Description": "Export list items to a CSV file.",
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
#       "Name": "OutputPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-OutputPath",
#       "Prompt": "Output CSV file path"
#     },
#     {
#       "Name": "Fields",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Fields",
#       "Prompt": "Comma-separated field names (default: Title)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [string]$Fields = "Title"
)

$fieldNames = $Fields.Split(",") | ForEach-Object { $_.Trim() }

$items = Get-PnPListItem -List $ListName -PageSize 500

$selectExpressions = @(@{L="Id";E={$_.Id}})
foreach ($field in $fieldNames) {
    $f = $field
    $selectExpressions += @{L=$f;E={$_.FieldValues[$f]}.GetNewClosure()}
}

$items | Select-Object $selectExpressions | Export-Csv -Path $OutputPath -NoTypeInformation
