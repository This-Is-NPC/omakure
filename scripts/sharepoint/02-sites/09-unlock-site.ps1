#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "unlock_site",
#   "Description": "Unlock a locked site collection.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Site collection URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl
)

Set-SPOSite -Identity $SiteUrl -LockState Unlock
