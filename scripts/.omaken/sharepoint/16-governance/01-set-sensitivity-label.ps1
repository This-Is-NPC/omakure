#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_sensitivity_label",
#   "Description": "Apply a sensitivity label to a SharePoint site.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "URL of the SharePoint site"
#     },
#     {
#       "Name": "LabelId",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-LabelId",
#       "Prompt": "GUID of the sensitivity label to apply"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [Parameter(Mandatory = $true)]
    [string]$LabelId
)

Set-SPOSite -Identity $SiteUrl -SensitivityLabel $LabelId
Write-Host "Sensitivity label '$LabelId' applied to: $SiteUrl"
