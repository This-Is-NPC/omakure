#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_site_column",
#   "Description": "Add a new site column.",
#   "Fields": [
#     {
#       "Name": "DisplayName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-DisplayName",
#       "Prompt": "Display name"
#     },
#     {
#       "Name": "InternalName",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-InternalName",
#       "Prompt": "Internal name"
#     },
#     {
#       "Name": "FieldType",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-FieldType",
#       "Prompt": "Field type",
#       "Choices": ["Text", "Note", "Number", "Currency", "DateTime", "Boolean", "Choice", "User", "URL"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$DisplayName,

    [Parameter(Mandatory = $true)]
    [string]$InternalName,

    [Parameter(Mandatory = $true)]
    [ValidateSet("Text", "Note", "Number", "Currency", "DateTime", "Boolean", "Choice", "User", "URL")]
    [string]$FieldType
)

Add-PnPField -DisplayName $DisplayName -InternalName $InternalName -Type $FieldType -Group "Custom Columns"
