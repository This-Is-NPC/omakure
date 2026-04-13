#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_folder",
#   "Description": "Create a new folder in a library.",
#   "Fields": [
#     {
#       "Name": "FolderName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-FolderName",
#       "Prompt": "Folder name"
#     },
#     {
#       "Name": "ParentFolder",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-ParentFolder",
#       "Prompt": "Parent folder (e.g. Shared Documents)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$FolderName,

    [Parameter(Mandatory = $true)]
    [string]$ParentFolder
)

Add-PnPFolder -Name $FolderName -Folder $ParentFolder
