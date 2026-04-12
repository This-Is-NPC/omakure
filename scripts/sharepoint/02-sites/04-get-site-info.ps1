#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "get_site_info",
#   "Description": "Get detailed information about a site collection.",
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

Get-SPOSite -Identity $SiteUrl -Detailed
