#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_communication_site",
#   "Description": "Create a modern communication site.",
#   "Fields": [
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Title",
#       "Prompt": "Site title"
#     },
#     {
#       "Name": "Url",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-Url",
#       "Prompt": "Full site URL"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Description",
#       "Prompt": "Site description"
#     },
#     {
#       "Name": "SiteDesign",
#       "Type": "string",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-SiteDesign",
#       "Prompt": "Site design template",
#       "Default": "Topic",
#       "Choices": ["Blank", "Topic", "Showcase"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Title,

    [Parameter(Mandatory = $true)]
    [string]$Url,

    [string]$Description = "",

    [ValidateSet("Blank", "Topic", "Showcase")]
    [string]$SiteDesign = "Topic"
)

$params = @{
    Type       = "CommunicationSite"
    Title      = $Title
    Url        = $Url
    SiteDesign = $SiteDesign
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

New-PnPSite @params
