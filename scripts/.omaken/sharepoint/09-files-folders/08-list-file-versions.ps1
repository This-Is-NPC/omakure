#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_file_versions",
#   "Description": "List all versions of a file.",
#   "Fields": [
#     {
#       "Name": "FileUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-FileUrl",
#       "Prompt": "Server-relative file URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$FileUrl
)

Get-PnPFileVersion -Url $FileUrl | Format-Table VersionLabel, Created, CreatedBy, Size
