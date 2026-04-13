#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "associate_site_to_hub",
#   "Description": "Associate a site to a hub site.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Site to associate"
#     },
#     {
#       "Name": "HubSiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-HubSiteUrl",
#       "Prompt": "Hub site URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [Parameter(Mandatory = $true)]
    [string]$HubSiteUrl
)

Add-SPOHubSiteAssociation -Site $SiteUrl -HubSite $HubSiteUrl
