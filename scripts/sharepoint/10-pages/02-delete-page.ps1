#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "delete_page",
#   "Description": "Delete a modern site page.",
#   "Fields": [
#     {
#       "Name": "PageName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-PageName",
#       "Prompt": "Page name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$PageName
)

Remove-PnPPage -Identity $PageName -Force
