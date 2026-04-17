#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_hub_sites",
#   "Description": "List all hub sites in the tenant.",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END

param()

Get-SPOHubSite | Format-Table SiteUrl, Title, SiteId
