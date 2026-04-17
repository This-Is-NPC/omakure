#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "create_term",
#   "Description": "Create a new term in a term set.",
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
#       "Name": "TermName",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-TermName",
#       "Prompt": "Term name"
#     },
#     {
#       "Name": "ParentTermId",
#       "Type": "string",
#       "Required": false,
#       "Order": 4,
#       "Arg": "-ParentTermId",
#       "Prompt": "Parent term GUID for child terms"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$TermGroupName,

    [Parameter(Mandatory = $true)]
    [string]$TermSetName,

    [Parameter(Mandatory = $true)]
    [string]$TermName,

    [string]$ParentTermId = ""
)

$params = @{
    Name      = $TermName
    TermSet   = $TermSetName
    TermGroup = $TermGroupName
    Lcid      = 1033
}

if ($ParentTermId -ne "") {
    $params["ParentTermId"] = $ParentTermId
}

New-PnPTerm @params
