#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_document_library",
#   "Description": "Create a new document library.",
#   "Fields": [
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Title",
#       "Prompt": "Library title"
#     },
#     {
#       "Name": "EnableVersioning",
#       "Type": "bool",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-EnableVersioning",
#       "Prompt": "Enable versioning",
#       "Default": "true"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Title,

    [bool]$EnableVersioning = $true
)

New-PnPList -Title $Title -Template DocumentLibrary -EnableVersioning:$EnableVersioning -OnQuickLaunch
