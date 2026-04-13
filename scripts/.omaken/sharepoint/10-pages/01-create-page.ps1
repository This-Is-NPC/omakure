#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_page",
#   "Description": "Create a new modern site page.",
#   "Fields": [
#     {
#       "Name": "PageName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-PageName",
#       "Prompt": "Page name (without .aspx)"
#     },
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Title",
#       "Prompt": "Title"
#     },
#     {
#       "Name": "LayoutType",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-LayoutType",
#       "Prompt": "Layout type",
#       "Choices": ["Article", "Home", "SingleWebPartAppPage"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$PageName,

    [Parameter(Mandatory = $true)]
    [string]$Title,

    [Parameter(Mandatory = $true)]
    [ValidateSet("Article", "Home", "SingleWebPartAppPage")]
    [string]$LayoutType
)

Add-PnPPage -Name $PageName -Title $Title -LayoutType $LayoutType
