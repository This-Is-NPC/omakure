#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "upload_file",
#   "Description": "Upload a file to a document library.",
#   "Fields": [
#     {
#       "Name": "LocalPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-LocalPath",
#       "Prompt": "Local file path"
#     },
#     {
#       "Name": "Folder",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Folder",
#       "Prompt": "Target folder (e.g. Shared Documents)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$LocalPath,

    [Parameter(Mandatory = $true)]
    [string]$Folder
)

Add-PnPFile -Path $LocalPath -Folder $Folder
