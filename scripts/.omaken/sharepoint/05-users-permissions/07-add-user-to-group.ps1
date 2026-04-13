#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_user_to_group",
#   "Description": "Add a user to a SharePoint group.",
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

Add-PnPGroupMember -LoginName $LoginName -Group $GroupName
