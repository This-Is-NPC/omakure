#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "reset_list_inheritance",
#   "Description": "Reset a list to inherit permissions from the site.",
#   "Fields": [
#     { "Name": "ListName", "Type": "string", "Required": true, "Order": 1, "Arg": "-ListName" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$ListName
)

Set-PnPList -Identity $ListName -ResetRoleInheritance
