#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "inactive_sites_report",
#   "Description": "Report sites that have not been modified within a specified number of days.",
#   "Fields": [
#     {
#       "Name": "DaysInactive",
#       "Type": "number",
#       "Required": false,
#       "Order": 1,
#       "Arg": "-DaysInactive",
#       "Default": "90",
#       "Prompt": "Number of days of inactivity threshold"
#     },
#     {
#       "Name": "OutputPath",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-OutputPath",
#       "Prompt": "Optional CSV file path to save the report"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $false)]
    [int]$DaysInactive = 90,

    [Parameter(Mandatory = $false)]
    [string]$OutputPath
)

$cutoff = (Get-Date).AddDays(-$DaysInactive)
$sites = Get-SPOSite -Limit All |
    Where-Object { $_.LastContentModifiedDate -lt $cutoff } |
    Select-Object Url, Title, LastContentModifiedDate, StorageUsageCurrent

Write-Host "Found $($sites.Count) inactive site(s) (no changes in $DaysInactive days)."

if ($OutputPath) {
    $sites | Export-Csv -Path $OutputPath -NoTypeInformation
    Write-Host "Report saved to: $OutputPath"
} else {
    $sites | Format-Table Url, Title, LastContentModifiedDate, StorageUsageCurrent -AutoSize
}
