#Requires -Version 5.1

# OMAKURE_SCHEMA_START
# {
#   "Name": "connect_pnp_interactive",
#   "Description": "Connect to a SharePoint site using interactive browser login.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Site URL (e.g. https://contoso.sharepoint.com/sites/MySite)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl
)

Connect-PnPOnline -Url $SiteUrl -Interactive
