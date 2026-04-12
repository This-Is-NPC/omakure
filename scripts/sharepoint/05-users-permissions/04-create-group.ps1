#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_group",
#   "Description": "Create a new SharePoint group.",
#   "Fields": [
#     { "Name": "GroupName", "Type": "string", "Required": true, "Order": 1, "Arg": "-GroupName", "Description": "Group name" },
#     { "Name": "Owner", "Type": "string", "Required": false, "Order": 2, "Arg": "-Owner", "Description": "Owner email" },
#     { "Name": "Description", "Type": "string", "Required": false, "Order": 3, "Arg": "-Description" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$GroupName,

    [string]$Owner = "",

    [string]$Description = ""
)

$params = @{
    Title = $GroupName
}

if ($Owner) {
    $params["Owner"] = $Owner
}

if ($Description) {
    $params["Description"] = $Description
}

New-PnPGroup @params
