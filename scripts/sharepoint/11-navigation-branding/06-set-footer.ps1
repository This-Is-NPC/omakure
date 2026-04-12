#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_footer",
#   "Description": "Configure the site footer.",
#   "Fields": [
#     {
#       "Name": "Enabled",
#       "Type": "bool",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Enabled",
#       "Prompt": "Enable footer"
#     },
#     {
#       "Name": "Title",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Title",
#       "Prompt": "Footer title"
#     },
#     {
#       "Name": "LogoUrl",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-LogoUrl",
#       "Prompt": "Footer logo URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [bool]$Enabled,

    [string]$Title = "",

    [string]$LogoUrl = ""
)

$params = @{
    Enabled = $Enabled
}

if ($Title -ne "") {
    $params["Title"] = $Title
}

if ($LogoUrl -ne "") {
    $params["LogoUrl"] = $LogoUrl
}

Set-PnPFooter @params
