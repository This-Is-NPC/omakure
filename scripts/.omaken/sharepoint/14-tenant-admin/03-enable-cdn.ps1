#Requires -Version 5.1

# Requires: Connect-SPOService (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "enable_cdn",
#   "Description": "Enable the SharePoint Online CDN for public or private assets.",
#   "Fields": [
#     {
#       "Name": "CdnType",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-CdnType",
#       "Prompt": "CDN type to enable",
#       "Choices": ["Public", "Private"]
#     },
#     {
#       "Name": "Origins",
#       "Type": "string",
#       "Required": false,
#       "Order": 2,
#       "Arg": "-Origins",
#       "Default": "*/CLIENTSIDEASSETS",
#       "Prompt": "CDN origin(s), comma-separated (e.g. */CLIENTSIDEASSETS)"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [ValidateSet("Public", "Private")]
    [string]$CdnType,

    [Parameter(Mandatory = $false)]
    [string]$Origins = "*/CLIENTSIDEASSETS"
)

Set-SPOTenantCdnEnabled -CdnType $CdnType -Enable $true
Write-Host "$CdnType CDN enabled."

foreach ($origin in ($Origins -split ",")) {
    $origin = $origin.Trim()
    Add-SPOTenantCdnOrigin -CdnType $CdnType -OriginUrl $origin
    Write-Host "Added CDN origin: $origin"
}
