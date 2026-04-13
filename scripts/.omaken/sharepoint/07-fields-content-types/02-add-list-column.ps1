#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_list_column",
#   "Description": "Add a new column to a list or library.",
#   "Fields": [
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ListName",
#       "Prompt": "List or library name"
#     },
#     {
#       "Name": "DisplayName",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-DisplayName",
#       "Prompt": "Display name"
#     },
#     {
#       "Name": "InternalName",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-InternalName",
#       "Prompt": "Internal name"
#     },
#     {
#       "Name": "FieldType",
#       "Type": "string",
#       "Required": true,
#       "Order": 4,
#       "Arg": "-FieldType",
#       "Prompt": "Field type",
#       "Choices": ["Text", "Note", "Number", "Currency", "DateTime", "Boolean", "Choice", "User", "URL"]
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [string]$DisplayName,

    [Parameter(Mandatory = $true)]
    [string]$InternalName,

    [Parameter(Mandatory = $true)]
    [ValidateSet("Text", "Note", "Number", "Currency", "DateTime", "Boolean", "Choice", "User", "URL")]
    [string]$FieldType
)

Add-PnPField -List $ListName -DisplayName $DisplayName -InternalName $InternalName -Type $FieldType -AddToDefaultView
