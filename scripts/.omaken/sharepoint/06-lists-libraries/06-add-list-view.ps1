#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_list_view",
#   "Description": "Add a new view to a list or library.",
#   "Fields": [
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ListName",
#       "Prompt": "List or library name"
#     },
#     {
#       "Name": "ViewTitle",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-ViewTitle",
#       "Prompt": "View name"
#     },
#     {
#       "Name": "Fields",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-Fields",
#       "Prompt": "Comma-separated field names"
#     },
#     {
#       "Name": "Query",
#       "Type": "string",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-Query",
#       "Prompt": "CAML query XML"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [string]$ViewTitle,

    [Parameter(Mandatory = $true)]
    [string]$Fields,

    [string]$Query = ""
)

$fieldArray = $Fields -split ","

$params = @{
    List   = $ListName
    Title  = $ViewTitle
    Fields = $fieldArray
}

if ($Query -ne "") {
    $params["Query"] = $Query
}

Add-PnPView @params
