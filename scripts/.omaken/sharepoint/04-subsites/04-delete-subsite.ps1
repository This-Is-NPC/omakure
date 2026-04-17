#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "delete_subsite",
#   "Description": "Delete a subsite.",
#   "Fields": [
#     {
#       "Name": "WebUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-WebUrl",
#       "Prompt": "Relative URL of the subsite"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$WebUrl
)

Remove-PnPWeb -Identity $WebUrl -Force
