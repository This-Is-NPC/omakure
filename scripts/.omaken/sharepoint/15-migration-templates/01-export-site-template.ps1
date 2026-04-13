#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "export_site_template",
#   "Description": "Export the current site as a PnP site template.",
#   "Fields": [
#     {
#       "Name": "OutputPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-OutputPath",
#       "Prompt": "Path to save the exported template (e.g. template.xml or template.pnp)"
#     },
#     {
#       "Name": "Handlers",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Handlers",
#       "Prompt": "Comma-separated list of handlers to include (leave blank for all)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$OutputPath,

    [Parameter(Mandatory = $false)]
    [string]$Handlers
)

if ($Handlers) {
    $handlerList = $Handlers -split "," | ForEach-Object { $_.Trim() }
    Get-PnPSiteTemplate -Out $OutputPath -Handlers $handlerList
} else {
    Get-PnPSiteTemplate -Out $OutputPath
}

Write-Host "Site template exported to: $OutputPath"
