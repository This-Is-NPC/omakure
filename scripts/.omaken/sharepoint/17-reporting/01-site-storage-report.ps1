#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "site_storage_report",
#   "Description": "Generate a report of storage usage across all SharePoint sites.",
#   "Fields": [
#     {
#       "Name": "OutputPath",
#       "Type": "string",
#       "Required": false,
#       "Order": 1,
#       "Arg": "-OutputPath",
#       "Prompt": "Optional CSV file path to save the report"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $false)]
    [string]$OutputPath
)

$sites = Get-SPOSite -Limit All | Select-Object Url, Title, StorageUsageCurrent, StorageQuota, LastContentModifiedDate

if ($OutputPath) {
    $sites | Export-Csv -Path $OutputPath -NoTypeInformation
    Write-Host "Storage report saved to: $OutputPath"
} else {
    $sites | Format-Table Url, Title, StorageUsageCurrent, StorageQuota, LastContentModifiedDate -AutoSize
}
