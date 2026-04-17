#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_list",
#   "Description": "Create a new SharePoint list.",
#   "Fields": [
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Title",
#       "Prompt": "List title"
#     },
#     {
#       "Name": "Template",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Template",
#       "Prompt": "List template",
#       "Choices": ["GenericList", "Announcements", "Contacts", "Events", "Tasks", "IssueTracking", "Links", "Survey"],
#       "Default": "GenericList"
#     },
#     {
#       "Name": "Url",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Url",
#       "Prompt": "Custom list URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Title,

    [ValidateSet("GenericList", "Announcements", "Contacts", "Events", "Tasks", "IssueTracking", "Links", "Survey")]
    [string]$Template = "GenericList",

    [string]$Url = ""
)

$params = @{
    Title    = $Title
    Template = $Template
    OnQuickLaunch = $true
}

if ($Url -ne "") {
    $params["Url"] = $Url
}

New-PnPList @params
