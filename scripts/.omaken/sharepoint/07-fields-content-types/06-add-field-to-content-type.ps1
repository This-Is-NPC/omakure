#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_field_to_content_type",
#   "Description": "Add an existing field to a content type.",
#   "Fields": [
#     {
#       "Name": "FieldName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-FieldName",
#       "Prompt": "Field internal name"
#     },
#     {
#       "Name": "ContentTypeName",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-ContentTypeName",
#       "Prompt": "Content type name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$FieldName,

    [Parameter(Mandatory = $true)]
    [string]$ContentTypeName
)

Add-PnPFieldToContentType -Field $FieldName -ContentType $ContentTypeName
