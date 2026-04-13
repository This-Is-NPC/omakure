#Requires -Version 5.1

# OMAKURE_SCHEMA_START
# {
#   "Name": "connect_pnp_certificate",
#   "Description": "Connect to SharePoint using Azure AD app certificate authentication.",
#   "Fields": [
#     {
#       "Name": "SiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SiteUrl",
#       "Prompt": "Site URL (e.g. https://contoso.sharepoint.com/sites/MySite)"
#     },
#     {
#       "Name": "ClientId",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-ClientId",
#       "Prompt": "Azure AD App Client ID"
#     },
#     {
#       "Name": "Tenant",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-Tenant",
#       "Prompt": "Tenant (e.g. contoso.onmicrosoft.com)"
#     },
#     {
#       "Name": "Thumbprint",
#       "Type": "string",
#       "Required": true,
#       "Order": 4,
#       "Arg": "-Thumbprint",
#       "Prompt": "Certificate thumbprint"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SiteUrl,

    [Parameter(Mandatory = $true)]
    [string]$ClientId,

    [Parameter(Mandatory = $true)]
    [string]$Tenant,

    [Parameter(Mandatory = $true)]
    [string]$Thumbprint
)

Connect-PnPOnline -Url $SiteUrl -ClientId $ClientId -Tenant $Tenant -Thumbprint $Thumbprint
