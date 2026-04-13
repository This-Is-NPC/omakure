#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_site_sharing",
#   "Description": "Set the external sharing capability for a site.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Site collection URL"
#     },
#     {
#       "Name": "SharingCapability",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-SharingCapability",
#       "Prompt": "Sharing capability level",
#       "Choices": ["Disabled", "ExistingExternalUserSharingOnly", "ExternalUserSharingOnly", "ExternalUserAndGuestSharing"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [Parameter(Mandatory = $true)]
    [ValidateSet("Disabled", "ExistingExternalUserSharingOnly", "ExternalUserSharingOnly", "ExternalUserAndGuestSharing")]
    [string]$SharingCapability
)

Set-SPOSite -Identity $SiteUrl -SharingCapability $SharingCapability
