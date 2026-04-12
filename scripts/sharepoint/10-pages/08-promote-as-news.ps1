#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "promote_as_news",
#   "Description": "Promote a page as a news article.",
#   "Fields": [
#     {
#       "Name": "PageName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-PageName",
#       "Prompt": "Page name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$PageName
)

Set-PnPPage -Identity $PageName -PromoteAs NewsArticle
