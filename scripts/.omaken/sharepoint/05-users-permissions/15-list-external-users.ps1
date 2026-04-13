#Requires -Version 5.1
# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "list_external_users",
#   "Description": "List external users in the tenant or a specific site.",
#   "Fields": [
#     { "Name": "SiteUrl", "Type": "string", "Required": false, "Order": 1, "Arg": "-SiteUrl", "Description": "Filter by site URL" },
#     { "Name": "Filter", "Type": "string", "Required": false, "Order": 2, "Arg": "-Filter", "Description": "Filter by email" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [string]$SiteUrl = "",

    [string]$Filter = ""
)

$params = @{}

if ($SiteUrl) {
    $params["SiteUrl"] = $SiteUrl
}

if ($Filter) {
    $params["Filter"] = $Filter
}

Get-SPOExternalUser @params | Format-Table DisplayName, Email, AcceptedAs, WhenCreated
