#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "get_list_items",
#   "Description": "Get items from a list.",
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
#       "Name": "PageSize",
#       "Type": "number",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-PageSize",
#       "Prompt": "Page size",
#       "Default": "100"
#     },
#     {
#       "Name": "Query",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Query",
#       "Prompt": "CAML query XML"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [int]$PageSize = 100,

    [string]$Query = ""
)

$params = @{
    List     = $ListName
    PageSize = $PageSize
}

if ($Query -ne "") {
    $params["Query"] = $Query
}

Get-PnPListItem @params | Format-Table Id, @{L="Title";E={$_.FieldValues.Title}}, @{L="Created";E={$_.FieldValues.Created}}, @{L="Modified";E={$_.FieldValues.Modified}}
