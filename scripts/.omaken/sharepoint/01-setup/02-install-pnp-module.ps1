#Requires -Version 5.1

# OMAKURE_SCHEMA_START
# {
#   "Name": "install_pnp_module",
#   "Description": "Install or update the PnP PowerShell module.",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END

param()

Install-Module -Name PnP.PowerShell -Force -AllowClobber
Get-Module -Name PnP.PowerShell -ListAvailable
