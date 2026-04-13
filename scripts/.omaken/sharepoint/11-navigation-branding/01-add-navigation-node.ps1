#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_navigation_node",
#   "Description": "Add a navigation link.",
#   "Fields": [
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Title",
#       "Prompt": "Link text"
#     },
#     {
#       "Name": "Url",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Url",
#       "Prompt": "Link URL"
#     },
#     {
#       "Name": "Location",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-Location",
#       "Prompt": "Navigation location",
#       "Choices": ["TopNavigationBar", "QuickLaunch", "Footer"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Title,

    [Parameter(Mandatory = $true)]
    [string]$Url,

    [Parameter(Mandatory = $true)]
    [ValidateSet("TopNavigationBar", "QuickLaunch", "Footer")]
    [string]$Location
)

Add-PnPNavigationNode -Location $Location -Title $Title -Url $Url
