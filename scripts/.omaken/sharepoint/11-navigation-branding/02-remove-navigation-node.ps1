#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "remove_navigation_node",
#   "Description": "Remove a navigation node by ID.",
#   "Fields": [
#     {
#       "Name": "NodeId",
#       "Type": "number",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-NodeId",
#       "Prompt": "Navigation node ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [int]$NodeId
)

Remove-PnPNavigationNode -Identity $NodeId -Force
