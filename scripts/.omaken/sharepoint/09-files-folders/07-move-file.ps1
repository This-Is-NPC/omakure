#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "move_file",
#   "Description": "Move a file to another location.",
#   "Fields": [
#     {
#       "Name": "SourceUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SourceUrl",
#       "Prompt": "Source server-relative URL"
#     },
#     {
#       "Name": "TargetUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-TargetUrl",
#       "Prompt": "Target server-relative URL"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SourceUrl,

    [Parameter(Mandatory = $true)]
    [string]$TargetUrl
)

Move-PnPFile -SourceUrl $SourceUrl -TargetUrl $TargetUrl -OverwriteIfAlreadyExists -Force
