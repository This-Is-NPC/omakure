#Requires -Version 5.1

# OMAKURE_SCHEMA_START
# {
#   "Name": "connect_spo_service",
#   "Description": "Connect to SharePoint Online admin center.",
#   "Fields": [
#     {
#       "Name": "AdminUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-AdminUrl",
#       "Prompt": "Admin center URL (e.g. https://contoso-admin.sharepoint.com)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$AdminUrl
)

Connect-SPOService -Url $AdminUrl
