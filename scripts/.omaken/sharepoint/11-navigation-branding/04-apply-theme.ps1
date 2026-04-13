#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "apply_theme",
#   "Description": "Apply a theme to the current site.",
#   "Fields": [
#     {
#       "Name": "ThemeName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ThemeName",
#       "Prompt": "Theme name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ThemeName
)

Set-PnPWebTheme -Theme $ThemeName
