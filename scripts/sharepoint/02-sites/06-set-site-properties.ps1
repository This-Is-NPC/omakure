#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_site_properties",
#   "Description": "Update site collection properties (title, owner, storage quota).",
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
#       "Name": "Title",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Title",
#       "Prompt": "New site title"
#     },
#     {
#       "Name": "Owner",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Owner",
#       "Prompt": "New owner email"
#     },
#     {
#       "Name": "StorageQuota",
#       "Type": "number",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-StorageQuota",
#       "Prompt": "Storage quota in MB"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [string]$Title = "",

    [string]$Owner = "",

    [int]$StorageQuota = 0
)

$params = @{
    Identity = $SiteUrl
}

if ($Title -ne "") {
    $params["Title"] = $Title
}

if ($Owner -ne "") {
    $params["Owner"] = $Owner
}

if ($StorageQuota -gt 0) {
    $params["StorageQuotaMB"] = $StorageQuota
}

Set-SPOSite @params
