#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "lock_site",
#   "Description": "Lock a site collection (read-only or no access).",
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
#       "Name": "LockState",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-LockState",
#       "Prompt": "Lock state",
#       "Choices": ["ReadOnly", "NoAccess"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [Parameter(Mandatory = $true)]
    [ValidateSet("ReadOnly", "NoAccess")]
    [string]$LockState
)

Set-SPOSite -Identity $SiteUrl -LockState $LockState
