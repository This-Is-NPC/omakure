#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_web_properties",
#   "Description": "Update properties of the current web.",
#   "Fields": [
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": false,
#       "Order": 1,
#       "Arg": "-Title",
#       "Prompt": "Web title"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Description",
#       "Prompt": "Web description"
#     },
#     {
#       "Name": "SiteLogoUrl",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-SiteLogoUrl",
#       "Prompt": "Logo URL (server-relative)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [string]$Title = "",

    [string]$Description = "",

    [string]$SiteLogoUrl = ""
)

$params = @{}

if ($Title -ne "") {
    $params["Title"] = $Title
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

if ($SiteLogoUrl -ne "") {
    $params["SiteLogoUrl"] = $SiteLogoUrl
}

Set-PnPWeb @params
