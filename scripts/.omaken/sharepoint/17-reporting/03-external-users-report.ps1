#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "external_users_report",
#   "Description": "Report external users across the tenant or for a specific site.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": false,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Optional: URL of a specific site to filter (leave blank for all sites)"
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
    [string]$SiteUrl,

    [Parameter(Mandatory = $false)]
    [string]$OutputPath
)

if ($SiteUrl) {
    $users = Get-SPOExternalUser -SiteUrl $SiteUrl -PageSize 200 |
        Select-Object DisplayName, Email, LoginName, AcceptedAs, WhenCreated
} else {
    $users = Get-SPOExternalUser -PageSize 200 |
        Select-Object DisplayName, Email, LoginName, AcceptedAs, WhenCreated
}

Write-Host "Found $($users.Count) external user(s)."

if ($OutputPath) {
    $users | Export-Csv -Path $OutputPath -NoTypeInformation
    Write-Host "Report saved to: $OutputPath"
} else {
    $users | Format-Table DisplayName, Email, AcceptedAs, WhenCreated -AutoSize
}
