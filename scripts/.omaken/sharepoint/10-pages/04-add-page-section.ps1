#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_page_section",
#   "Description": "Add a section to a modern page.",
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
#       "Name": "SectionTemplate",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-SectionTemplate",
#       "Prompt": "Section template",
#       "Choices": ["OneColumn", "TwoColumn", "TwoColumnLeft", "TwoColumnRight", "ThreeColumn", "OneColumnFullWidth"]
#     },
#     {
#       "Name": "Order",
#       "Type": "number",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Order",
#       "Prompt": "Section order position"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$PageName,

    [Parameter(Mandatory = $true)]
    [ValidateSet("OneColumn", "TwoColumn", "TwoColumnLeft", "TwoColumnRight", "ThreeColumn", "OneColumnFullWidth")]
    [string]$SectionTemplate,

    [int]$Order = 0
)

$params = @{
    Page            = $PageName
    SectionTemplate = $SectionTemplate
}

if ($Order -gt 0) {
    $params["Order"] = $Order
}

Add-PnPPageSection @params
