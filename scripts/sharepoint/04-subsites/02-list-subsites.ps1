#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_subsites",
#   "Description": "List all subsites under the current site.",
#   "Fields": [
#     {
#       "Name": "Recurse",
#       "Type": "bool",
#       "Required": false,
#       "Order": 1,
#       "Arg": "-Recurse",
#       "Prompt": "Include nested subsites",
#       "Default": "false"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [bool]$Recurse = $false
)

if ($Recurse) {
    Get-PnPSubWeb -Recurse | Format-Table Title, Url
} else {
    Get-PnPSubWeb | Format-Table Title, Url
}
