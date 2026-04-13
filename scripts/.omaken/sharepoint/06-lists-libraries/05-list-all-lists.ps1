#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_all_lists",
#   "Description": "List all lists and libraries in the current site.",
#   "Fields": []
# }
# OMAKURE_SCHEMA_END

param()

Get-PnPList | Where-Object { -not $_.Hidden } | Format-Table Title, ItemCount, BaseTemplate, LastItemModifiedDate
