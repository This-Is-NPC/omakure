#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_term_group",
#   "Description": "Create a new term group in the term store.",
#   "Fields": [
#     {
#       "Name": "GroupName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-GroupName",
#       "Prompt": "Group name"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Description",
#       "Prompt": "Description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$GroupName,

    [string]$Description = ""
)

$params = @{
    Name = $GroupName
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

New-PnPTermGroup @params
