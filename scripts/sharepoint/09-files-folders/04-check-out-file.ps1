#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "check_out_file",
#   "Description": "Check out a file for editing.",
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

Set-PnPFileCheckedOut -Url $FileUrl
