#Requires -Version 5.1

# OMAKURE_SCHEMA_START
# {
#   "Name": "install_spo_module",
#   "Description": "Install or update the SharePoint Online Management Shell module.",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END

param()

Install-Module -Name Microsoft.Online.SharePoint.PowerShell -Force -AllowClobber
Get-Module -Name Microsoft.Online.SharePoint.PowerShell -ListAvailable
