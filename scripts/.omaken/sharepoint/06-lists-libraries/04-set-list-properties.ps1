#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_list_properties",
#   "Description": "Update list or library settings.",
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
#       "Name": "Title",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Title",
#       "Prompt": "New title"
#     },
#     {
#       "Name": "EnableVersioning",
#       "Type": "bool",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-EnableVersioning",
#       "Prompt": "Enable versioning"
#     },
#     {
#       "Name": "MajorVersions",
#       "Type": "number",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-MajorVersions",
#       "Prompt": "Max major versions"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [string]$Title = "",

    [bool]$EnableVersioning,

    [int]$MajorVersions = 0
)

$params = @{
    Identity = $ListName
}

if ($Title -ne "") {
    $params["Title"] = $Title
}

if ($PSBoundParameters.ContainsKey("EnableVersioning")) {
    $params["EnableVersioning"] = $EnableVersioning
}

if ($MajorVersions -gt 0) {
    $params["MajorVersionLimit"] = $MajorVersions
}

Set-PnPList @params
