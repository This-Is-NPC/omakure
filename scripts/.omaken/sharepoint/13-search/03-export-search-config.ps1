#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "export_search_config",
#   "Description": "Export the search configuration for the current site or tenant.",
#   "Fields": [
#     {
#       "Name": "OutputPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-OutputPath",
#       "Prompt": "Path to save the exported search configuration XML"
#     },
#     {
#       "Name": "Scope",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Scope",
#       "Default": "Site",
#       "Prompt": "Scope of the search configuration export",
#       "Choices": ["Site", "SPSite", "Subscription"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $false)]
    [ValidateSet("Site", "SPSite", "Subscription")]
    [string]$Scope = "Site"
)

Export-PnPSearchConfiguration -Scope $Scope -Path $OutputPath
Write-Host "Search configuration exported to: $OutputPath"
