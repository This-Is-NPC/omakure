#Requires -Version 5.1
# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_groups",
#   "Description": "List all SharePoint groups in a site.",
#   "Fields": [
#     { "Name": "SiteUrl", "Type": "string", "Required": true, "Order": 1, "Arg": "-SiteUrl" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$SiteUrl
)

Get-SPOSiteGroup -Site $SiteUrl | Format-Table Title, OwnerTitle, Roles
