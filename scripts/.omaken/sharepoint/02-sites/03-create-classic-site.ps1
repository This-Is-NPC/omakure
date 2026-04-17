#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_classic_site",
#   "Description": "Create a classic site collection.",
#   "Fields": [
#     {
#       "Name": "Url",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Url",
#       "Prompt": "Site collection URL"
#     },
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Title",
#       "Prompt": "Site title"
#     },
#     {
#       "Name": "Owner",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-Owner",
#       "Prompt": "Owner email"
#     },
#     {
#       "Name": "StorageQuota",
#       "Type": "number",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-StorageQuota",
#       "Prompt": "Storage quota in MB",
#       "Default": "1024"
#     },
#     {
#       "Name": "Template",
#       "Type": "string",
#       "Required": false,
#       "Order": 5,
#       "Arg": "-Template",
#       "Prompt": "Site template",
#       "Default": "STS#3"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Url,

    [Parameter(Mandatory = $true)]
    [string]$Title,

    [Parameter(Mandatory = $true)]
    [string]$Owner,

    [int]$StorageQuota = 1024,

    [string]$Template = "STS#3"
)

New-SPOSite -Url $Url -Title $Title -Owner $Owner -StorageQuota $StorageQuota -Template $Template
