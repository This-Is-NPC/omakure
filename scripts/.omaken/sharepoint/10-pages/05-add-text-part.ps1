#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_page_text",
#   "Description": "Add a text web part to a page.",
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
#       "Name": "Text",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Text",
#       "Prompt": "HTML text content"
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
    [string]$Text,

    [Parameter(Mandatory = $true)]
    [int]$Section,

    [Parameter(Mandatory = $true)]
    [int]$Column
)

Add-PnPPageTextPart -Page $PageName -Text $Text -Section $Section -Column $Column
