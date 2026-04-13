#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "download_file",
#   "Description": "Download a file from SharePoint.",
#   "Fields": [
#     {
#       "Name": "ServerRelativeUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ServerRelativeUrl",
#       "Prompt": "Server-relative file URL"
#     },
#     {
#       "Name": "LocalPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-LocalPath",
#       "Prompt": "Local download directory"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ServerRelativeUrl,

    [Parameter(Mandatory = $true)]
    [string]$LocalPath
)

$fileName = Split-Path $ServerRelativeUrl -Leaf
Get-PnPFile -Url $ServerRelativeUrl -Path $LocalPath -FileName $fileName -AsFile
