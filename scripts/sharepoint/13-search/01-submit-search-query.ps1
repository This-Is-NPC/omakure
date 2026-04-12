#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "submit_search_query",
#   "Description": "Submit a search query against the SharePoint search index.",
#   "Fields": [
#     {
#       "Name": "Query",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-Query",
#       "Prompt": "Enter the search query (KQL)"
#     },
#     {
#       "Name": "MaxResults",
#       "Type": "number",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-MaxResults",
#       "Default": "10",
#       "Prompt": "Maximum number of results to return"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$Query,

    [Parameter(Mandatory = $false)]
    [int]$MaxResults = 10
)

Submit-PnPSearchQuery -Query $Query -MaxResults $MaxResults | Select-Object -ExpandProperty ResultRows | Format-Table Title, Path, LastModifiedTime
