#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "import_taxonomy",
#   "Description": "Import taxonomy from a file.",
#   "Fields": [
#     {
#       "Name": "InputPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-InputPath",
#       "Prompt": "Input file path"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$InputPath
)

Import-PnPTaxonomy -Path $InputPath
