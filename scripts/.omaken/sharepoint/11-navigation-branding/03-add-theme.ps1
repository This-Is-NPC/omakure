#Requires -Version 5.1
# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_tenant_theme",
#   "Description": "Add a custom theme to the tenant.",
#   "Fields": [
#     {
#       "Name": "ThemeName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ThemeName",
#       "Prompt": "Theme name"
#     },
#     {
#       "Name": "PaletteJson",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-PaletteJson",
#       "Prompt": "Theme palette as JSON string"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ThemeName,

    [Parameter(Mandatory = $true)]
    [string]$PaletteJson
)

$palette = $PaletteJson | ConvertFrom-Json -AsHashtable
Add-SPOTheme -Name $ThemeName -Palette $palette -IsInverted $false
