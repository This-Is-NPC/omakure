#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_content_type",
#   "Description": "Create a new content type.",
#   "Fields": [
#     {
#       "Name": "Name",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Name",
#       "Prompt": "Content type name"
#     },
#     {
#       "Name": "ParentId",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-ParentId",
#       "Prompt": "Parent content type ID (default: 0x01 Item)",
#       "Default": "0x01"
#     },
#     {
#       "Name": "Group",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Group",
#       "Prompt": "Content type group name"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-Description",
#       "Prompt": "Description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Name,

    [string]$ParentId = "0x01",

    [string]$Group = "",

    [string]$Description = ""
)

$params = @{
    Name            = $Name
    ContentTypeId   = $ParentId
}

if ($Group -ne "") {
    $params["Group"] = $Group
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

Add-PnPContentType @params
