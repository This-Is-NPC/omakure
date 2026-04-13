#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_page_web_part",
#   "Description": "Add a built-in web part to a page.",
#   "Fields": [
#     {
#       "Name": "PageName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-PageName",
#       "Prompt": "Page name"
#     },
#     {
#       "Name": "WebPartType",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-WebPartType",
#       "Prompt": "Web part type",
#       "Choices": ["Text", "Image", "Hero", "News", "QuickLinks", "List", "Events", "People", "ContentRollup", "SiteActivity"]
#     },
#     {
#       "Name": "Section",
#       "Type": "number",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-Section",
#       "Prompt": "Section number"
#     },
#     {
#       "Name": "Column",
#       "Type": "number",
#       "Required": true,
#       "Order": 4,
#       "Arg": "-Column",
#       "Prompt": "Column number"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$PageName,

    [Parameter(Mandatory = $true)]
    [ValidateSet("Text", "Image", "Hero", "News", "QuickLinks", "List", "Events", "People", "ContentRollup", "SiteActivity")]
    [string]$WebPartType,

    [Parameter(Mandatory = $true)]
    [int]$Section,

    [Parameter(Mandatory = $true)]
    [int]$Column
)

Add-PnPPageWebPart -Page $PageName -DefaultWebPartType $WebPartType -Section $Section -Column $Column
