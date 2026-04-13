#Requires -Version 5.1

# Requires: Connect-ExchangeOnline or Global Admin (uses Search-UnifiedAuditLog)

# OMAKURE_SCHEMA_START
# {
#   "Name": "search_audit_logs",
#   "Description": "Search the Microsoft 365 unified audit log for SharePoint activity.",
#   "Fields": [
#     {
#       "Name": "StartDate",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-StartDate",
#       "Prompt": "Start date for the audit log search (YYYY-MM-DD)"
#     },
#     {
#       "Name": "EndDate",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-EndDate",
#       "Prompt": "End date for the audit log search (YYYY-MM-DD)"
#     },
#     {
#       "Name": "Operations",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Operations",
#       "Prompt": "Comma-separated list of operations to filter (leave blank for all SharePoint operations)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$StartDate,

    [Parameter(Mandatory = $true)]
    [string]$EndDate,

    [Parameter(Mandatory = $false)]
    [string]$Operations
)

$params = @{
    StartDate  = [datetime]$StartDate
    EndDate    = [datetime]$EndDate
    RecordType = "SharePoint"
    ResultSize = 5000
}

if ($Operations) {
    $params["Operations"] = $Operations -split "," | ForEach-Object { $_.Trim() }
}

$results = Search-UnifiedAuditLog @params
Write-Host "Found $($results.Count) audit log entries."
$results | Select-Object CreationDate, UserIds, Operations, AuditData | Format-Table -AutoSize
