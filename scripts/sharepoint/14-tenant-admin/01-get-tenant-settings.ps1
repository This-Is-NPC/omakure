#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "get_tenant_settings",
#   "Description": "Retrieve and display key SharePoint Online tenant settings.",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END

param()

Get-SPOTenant | Format-List SharingCapability, DefaultSharingLinkType, RequireAnonymousLinksExpireInDays, StorageQuota, StorageQuotaAllocated, OneDriveStorageQuota, SignInAccelerationDomain, UsePersistentCookiesForExplorerView
