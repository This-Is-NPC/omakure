#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "upload_app_package",
#   "Description": "Upload and deploy an app package to the tenant or site app catalog.",
#   "Fields": [
#     {
#       "Name": "AppPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-AppPath",
#       "Prompt": "Path to the .sppkg app package file"
#     },
#     {
#       "Name": "Scope",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Scope",
#       "Default": "Tenant",
#       "Prompt": "App catalog scope",
#       "Choices": ["Tenant", "Site"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$AppPath,

    [Parameter(Mandatory = $false)]
    [ValidateSet("Tenant", "Site")]
    [string]$Scope = "Tenant"
)

$app = Add-PnPApp -Path $AppPath -Scope $Scope -Publish
Write-Host "App uploaded and published: $($app.Title) (ID: $($app.Id))"
