#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "permission_audit_report",
#   "Description": "Audit permissions on the current site, lists, and items.",
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

$report = @()
$web = Get-PnPWeb -Includes RoleAssignments, RoleAssignments.Member, RoleAssignments.RoleDefinitionBindings

foreach ($assignment in $web.RoleAssignments) {
    foreach ($role in $assignment.RoleDefinitionBindings) {
        $report += [PSCustomObject]@{
            Scope      = "Web"
            Object     = $web.Url
            Principal  = $assignment.Member.LoginName
            Role       = $role.Name
        }
    }
}

$lists = Get-PnPList | Where-Object { $_.HasUniqueRoleAssignments }
foreach ($list in $lists) {
    $listDetail = Get-PnPList -Identity $list.Id -Includes RoleAssignments, RoleAssignments.Member, RoleAssignments.RoleDefinitionBindings
    foreach ($assignment in $listDetail.RoleAssignments) {
        foreach ($role in $assignment.RoleDefinitionBindings) {
            $report += [PSCustomObject]@{
                Scope     = "List"
                Object    = $list.Title
                Principal = $assignment.Member.LoginName
                Role      = $role.Name
            }
        }
    }
}

Write-Host "Found $($report.Count) permission entries."

if ($OutputPath) {
    $report | Export-Csv -Path $OutputPath -NoTypeInformation
    Write-Host "Report saved to: $OutputPath"
} else {
    $report | Format-Table Scope, Object, Principal, Role -AutoSize
}
