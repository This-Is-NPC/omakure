#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_site_logo",
#   "Description": "Set the site logo.",
#   "Fields": [
#     {
#       "Name": "LogoUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-LogoUrl",
#       "Prompt": "Logo URL or server-relative path"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$LogoUrl
)

Set-PnPWeb -SiteLogoUrl $LogoUrl
