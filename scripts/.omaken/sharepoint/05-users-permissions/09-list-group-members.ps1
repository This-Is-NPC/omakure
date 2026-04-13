#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_group_members",
#   "Description": "List all members of a SharePoint group.",
#   "Fields": [
#     { "Name": "GroupName", "Type": "string", "Required": true, "Order": 1, "Arg": "-GroupName" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$GroupName
)

Get-PnPGroupMember -Group $GroupName | Format-Table Title, Email, LoginName
