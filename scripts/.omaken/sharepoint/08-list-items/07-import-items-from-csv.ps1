#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "import_items_from_csv",
#   "Description": "Import items from a CSV file into a list.",
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
#       "Name": "CsvPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-CsvPath",
#       "Prompt": "Path to CSV file"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [string]$CsvPath
)

$rows = Import-Csv $CsvPath

foreach ($row in $rows) {
    $values = @{}
    foreach ($prop in $row.PSObject.Properties) {
        $values[$prop.Name] = $prop.Value
    }
    Add-PnPListItem -List $ListName -Values $values
}
