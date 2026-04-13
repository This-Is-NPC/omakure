#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_sharing_policy",
#   "Description": "Set the tenant-wide external sharing capability for SharePoint and OneDrive.",
#   "Fields": [
#     {
#       "Name": "SharingCapability",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SharingCapability",
#       "Prompt": "Sharing capability level to apply",
#       "Choices": ["Disabled", "ExistingExternalUserSharingOnly", "ExternalUserSharingOnly", "ExternalUserAndGuestSharing"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Disabled", "ExistingExternalUserSharingOnly", "ExternalUserSharingOnly", "ExternalUserAndGuestSharing")]
    [string]$SharingCapability
)

Set-SPOTenant -SharingCapability $SharingCapability
Write-Host "Tenant sharing capability set to: $SharingCapability"
