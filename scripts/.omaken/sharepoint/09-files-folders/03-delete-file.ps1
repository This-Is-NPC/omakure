#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "delete_file",
#   "Description": "Delete a file (moves to recycle bin).",
#   "Fields": [
#     {
#       "Name": "ServerRelativeUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ServerRelativeUrl",
#       "Prompt": "Server-relative file URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ServerRelativeUrl
)

Remove-PnPFile -ServerRelativeUrl $ServerRelativeUrl -Recycle -Force
