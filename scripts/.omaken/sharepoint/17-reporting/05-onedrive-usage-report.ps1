#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "onedrive_usage_report",
#   "Description": "Report storage usage across all OneDrive for Business sites.",
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

$sites = Get-SPOSite -IncludePersonalSite $true -Limit All -Filter "Url -like '-my.sharepoint.com/personal/'" |
    Select-Object Url, Owner, StorageUsageCurrent, StorageQuota, LastContentModifiedDate

Write-Host "Found $($sites.Count) OneDrive site(s)."

if ($OutputPath) {
    $sites | Export-Csv -Path $OutputPath -NoTypeInformation
    Write-Host "Report saved to: $OutputPath"
} else {
    $sites | Format-Table Url, Owner, StorageUsageCurrent, StorageQuota, LastContentModifiedDate -AutoSize
}
