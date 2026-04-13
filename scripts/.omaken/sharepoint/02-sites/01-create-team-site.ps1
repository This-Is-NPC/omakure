#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_team_site",
#   "Description": "Create a modern team site connected to a Microsoft 365 group.",
#   "Fields": [
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Title",
#       "Prompt": "Site title"
#     },
#     {
#       "Name": "Alias",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Alias",
#       "Prompt": "Site alias (URL slug)"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Description",
#       "Prompt": "Site description"
#     },
#     {
#       "Name": "IsPublic",
#       "Type": "bool",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-IsPublic",
#       "Prompt": "Make the group public",
#       "Default": "false"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Title,

    [Parameter(Mandatory = $true)]
    [string]$Alias,

    [string]$Description = "",

    [bool]$IsPublic = $false
)

$params = @{
    Type  = "TeamSite"
    Title = $Title
    Alias = $Alias
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

if ($IsPublic) {
    $params["IsPublic"] = $true
}

New-PnPSite @params
