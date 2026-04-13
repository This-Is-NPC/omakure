#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "delete_folder",
#   "Description": "Delete a folder (moves to recycle bin).",
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
#       "Prompt": "Parent folder"
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

Remove-PnPFolder -Name $FolderName -Folder $ParentFolder -Recycle -Force
