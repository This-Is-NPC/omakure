#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_site_admins",
#   "Description": "List all site collection administrators.",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END

param()

Get-PnPSiteCollectionAdmin | Format-Table Title, Email, LoginName
