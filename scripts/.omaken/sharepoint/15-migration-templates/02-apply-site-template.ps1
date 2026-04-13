#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "apply_site_template",
#   "Description": "Apply a PnP site template to the current site.",
#   "Fields": [
#     {
#       "Name": "TemplatePath",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-TemplatePath",
#       "Prompt": "Path to the PnP template file (.xml or .pnp)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$TemplatePath
)

Invoke-PnPSiteTemplate -Path $TemplatePath
Write-Host "Site template applied from: $TemplatePath"
