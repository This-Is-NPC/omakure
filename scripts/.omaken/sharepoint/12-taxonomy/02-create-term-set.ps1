#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_term_set",
#   "Description": "Create a new term set in a term group.",
#   "Fields": [
#     {
#       "Name": "TermGroupName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-TermGroupName",
#       "Prompt": "Term group name"
#     },
#     {
#       "Name": "TermSetName",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-TermSetName",
#       "Prompt": "Term set name"
#     },
#     {
#       "Name": "Description",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Description",
#       "Prompt": "Description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$TermGroupName,

    [Parameter(Mandatory = $true)]
    [string]$TermSetName,

    [string]$Description = ""
)

$params = @{
    Name      = $TermSetName
    TermGroup = $TermGroupName
    Lcid      = 1033
}

if ($Description -ne "") {
    $params["Description"] = $Description
}

New-PnPTermSet @params
