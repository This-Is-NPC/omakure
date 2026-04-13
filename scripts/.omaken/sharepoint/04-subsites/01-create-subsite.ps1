#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_subsite",
#   "Description": "Create a new subsite under the current site.",
#   "Fields": [
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Title",
#       "Prompt": "Subsite title"
#     },
#     {
#       "Name": "Url",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Url",
#       "Prompt": "Relative URL (e.g. subsite1)"
#     },
#     {
#       "Name": "Template",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Template",
#       "Prompt": "Site template",
#       "Default": "STS#3"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-Description",
#       "Prompt": "Subsite description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Title,

    [Parameter(Mandatory = $true)]
    [string]$Url,

    [string]$Template = "STS#3",

    [string]$Description = ""
)

$params = @{
    Title    = $Title
    Url      = $Url
    Template = $Template
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

New-PnPWeb @params
