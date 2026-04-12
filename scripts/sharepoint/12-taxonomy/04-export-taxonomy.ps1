#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "export_taxonomy",
#   "Description": "Export the term store to a file.",
#   "Fields": [
#     {
#       "Name": "OutputPath",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-OutputPath",
#       "Prompt": "Output file path"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$OutputPath
)

Export-PnPTaxonomy -Path $OutputPath
