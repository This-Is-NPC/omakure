#Requires -Version 5.1
# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "remove_site_admin",
#   "Description": "Remove a site collection administrator.",
#   "Fields": [
#     { "Name": "SiteUrl", "Type": "string", "Required": true, "Order": 1, "Arg": "-SiteUrl" },
#     { "Name": "LoginName", "Type": "string", "Required": true, "Order": 2, "Arg": "-LoginName", "Description": "User email" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$SiteUrl,

    [Parameter(Mandatory=$true)]
    [string]$LoginName
)

Set-SPOUser -Site $SiteUrl -LoginName $LoginName -IsSiteCollectionAdmin $false
