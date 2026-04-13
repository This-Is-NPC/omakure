#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "restore_file_version",
#   "Description": "Restore a specific version of a file.",
#   "Fields": [
#     {
#       "Name": "FileUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-FileUrl",
#       "Prompt": "Server-relative file URL"
#     },
#     {
#       "Name": "VersionLabel",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-VersionLabel",
#       "Prompt": "Version label (e.g. 2.0)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$FileUrl,

    [Parameter(Mandatory = $true)]
    [string]$VersionLabel
)

Restore-PnPFileVersion -Url $FileUrl -Identity $VersionLabel -Force
