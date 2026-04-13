#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "request_reindex",
#   "Description": "Request a re-index of a list or the entire web.",
#   "Fields": [
#     {
#       "Name": "Scope",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Scope",
#       "Prompt": "Scope to reindex",
#       "Choices": ["Web", "List"]
#     },
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-ListName",
#       "Prompt": "List name (required when Scope is List)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Web", "List")]
    [string]$Scope,

    [Parameter(Mandatory = $false)]
    [string]$ListName
)

if ($Scope -eq "List") {
    if (-not $ListName) {
        Write-Error "ListName is required when Scope is List."
        exit 1
    }
    Request-PnPReIndexList -Identity $ListName
    Write-Host "Re-index requested for list: $ListName"
} else {
    Request-PnPReIndexWeb
    Write-Host "Re-index requested for the current web."
}
