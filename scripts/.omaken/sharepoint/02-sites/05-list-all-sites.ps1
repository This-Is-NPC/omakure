#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_all_sites",
#   "Description": "List all site collections in the tenant.",
#   "Fields": [
#     {
#       "Name": "Filter",
#       "Type": "string",
#       "Required": false,
#       "Order": 1,
#       "Arg": "-Filter",
#       "Prompt": "URL filter pattern"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [string]$Filter = ""
)

if ($Filter -ne "") {
    Get-SPOSite -Limit All -Filter "Url -like '$Filter'"
} else {
    Get-SPOSite -Limit All | Format-Table Url, Title, StorageUsageCurrent
}
