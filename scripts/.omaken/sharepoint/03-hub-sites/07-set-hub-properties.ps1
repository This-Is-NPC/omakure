#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_hub_properties",
#   "Description": "Update hub site properties.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Site URL"
#     },
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Title",
#       "Prompt": "Hub title"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Description",
#       "Prompt": "Hub description"
#     },
#     {
#       "Name": "LogoUrl",
#       "Type": "string",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-LogoUrl",
#       "Prompt": "Logo URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [string]$Title = "",

    [string]$Description = "",

    [string]$LogoUrl = ""
)

$params = @{
    Identity = $SiteUrl
}

if ($Title -ne "") {
    $params["Title"] = $Title
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

if ($LogoUrl -ne "") {
    $params["LogoUrl"] = $LogoUrl
}

Set-SPOHubSite @params
