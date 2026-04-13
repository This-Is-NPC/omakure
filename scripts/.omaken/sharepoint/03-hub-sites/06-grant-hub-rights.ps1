#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "grant_hub_rights",
#   "Description": "Grant users the right to associate sites to a hub.",
#   "Fields": [
#     {
#       "Name": "HubSiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-HubSiteUrl",
#       "Prompt": "Hub site URL"
#     },
#     {
#       "Name": "Principals",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Principals",
#       "Prompt": "Comma-separated emails"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$HubSiteUrl,

    [Parameter(Mandatory = $true)]
    [string]$Principals
)

$users = $Principals -split ","
Grant-SPOHubSiteRights -Identity $HubSiteUrl -Principals $users -Rights Join
