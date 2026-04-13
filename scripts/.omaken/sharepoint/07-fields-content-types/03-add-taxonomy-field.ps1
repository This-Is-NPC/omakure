#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_taxonomy_field",
#   "Description": "Add a managed metadata (taxonomy) column.",
#   "Fields": [
#     {
#       "Name": "DisplayName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-DisplayName",
#       "Prompt": "Display name"
#     },
#     {
#       "Name": "InternalName",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-InternalName",
#       "Prompt": "Internal name"
#     },
#     {
#       "Name": "TermSetPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-TermSetPath",
#       "Prompt": "Term set path (e.g. Group|TermSet)"
#     },
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-ListName",
#       "Prompt": "List name (omit for site column)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$DisplayName,

    [Parameter(Mandatory = $true)]
    [string]$InternalName,

    [Parameter(Mandatory = $true)]
    [string]$TermSetPath,

    [string]$ListName = ""
)

$params = @{
    DisplayName  = $DisplayName
    InternalName = $InternalName
    TermSetPath  = $TermSetPath
}

if ($ListName -ne "") {
    $params["List"] = $ListName
}

Add-PnPTaxonomyField @params
