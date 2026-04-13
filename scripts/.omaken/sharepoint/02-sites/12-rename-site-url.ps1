#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "rename_site_url",
#   "Description": "Rename a site collection URL.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Current site URL"
#     },
#     {
#       "Name": "NewSiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-NewSiteUrl",
#       "Prompt": "New site URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [Parameter(Mandatory = $true)]
    [string]$NewSiteUrl
)

Start-SPOSiteRename -Identity $SiteUrl -NewSiteUrl $NewSiteUrl
