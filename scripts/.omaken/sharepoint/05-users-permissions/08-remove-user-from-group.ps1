#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "remove_user_from_group",
#   "Description": "Remove a user from a SharePoint group.",
#   "Fields": [
#     { "Name": "LoginName", "Type": "string", "Required": true, "Order": 1, "Arg": "-LoginName", "Description": "User email" },
#     { "Name": "GroupName", "Type": "string", "Required": true, "Order": 2, "Arg": "-GroupName", "Description": "Group name" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$LoginName,

    [Parameter(Mandatory=$true)]
    [string]$GroupName
)

Remove-PnPGroupMember -LoginName $LoginName -Group $GroupName
