#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "delete_group",
#   "Description": "Delete a SharePoint group.",
#   "Fields": [
#     { "Name": "GroupName", "Type": "string", "Required": true, "Order": 1, "Arg": "-GroupName" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$GroupName
)

Remove-PnPGroup -Identity $GroupName -Force
