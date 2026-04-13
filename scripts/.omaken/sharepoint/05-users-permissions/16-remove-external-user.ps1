#Requires -Version 5.1
# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "remove_external_user",
#   "Description": "Remove an external user by unique ID.",
#   "Fields": [
#     { "Name": "UniqueId", "Type": "string", "Required": true, "Order": 1, "Arg": "-UniqueId", "Description": "External user unique ID" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$UniqueId
)

Remove-SPOExternalUser -UniqueIDs $UniqueId
