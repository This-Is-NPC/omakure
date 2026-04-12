# OMAKURE_SCHEMA_START
# {
#   "Name": "emergency_create_civic_address",
#   "Description": "Create civic address for emergency services",
#   "Tags": ["teams", "emergency", "lis", "create"],
#   "Fields": [
#     {
#       "Name": "house_number",
#       "Type": "string",
#       "Required": true,
#       "Description": "House number"
#     },
#     {
#       "Name": "street_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Street name"
#     },
#     {
#       "Name": "city",
#       "Type": "string",
#       "Required": true,
#       "Description": "City"
#     },
#     {
#       "Name": "state_or_province",
#       "Type": "string",
#       "Required": true,
#       "Description": "State or province"
#     },
#     {
#       "Name": "postal_code",
#       "Type": "string",
#       "Required": true,
#       "Description": "Postal code"
#     },
#     {
#       "Name": "country_or_region",
#       "Type": "string",
#       "Required": true,
#       "Default": "US",
#       "Description": "Country or region code"
#     },
#     {
#       "Name": "company_name",
#       "Type": "string",
#       "Required": false,
#       "Description": "Company name"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$HouseNumber = ""
$StreetName = ""
$City = ""
$StateOrProvince = ""
$PostalCode = ""
$CountryOrRegion = "US"
$CompanyName = ""
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--house_number" { $HouseNumber = $args[++$i] }
    "--street_name" { $StreetName = $args[++$i] }
    "--city" { $City = $args[++$i] }
    "--state_or_province" { $StateOrProvince = $args[++$i] }
    "--postal_code" { $PostalCode = $args[++$i] }
    "--country_or_region" { $CountryOrRegion = $args[++$i] }
    "--company_name" { $CompanyName = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($HouseNumber -eq "") { Write-Error "--house_number is required"; exit 1 }
if ($StreetName -eq "") { Write-Error "--street_name is required"; exit 1 }
if ($City -eq "") { Write-Error "--city is required"; exit 1 }
if ($StateOrProvince -eq "") { Write-Error "--state_or_province is required"; exit 1 }
if ($PostalCode -eq "") { Write-Error "--postal_code is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlineliscivicaddress?view=teams-ps
$params = @{
  HouseNumber     = $HouseNumber
  StreetName      = $StreetName
  City            = $City
  StateOrProvince = $StateOrProvince
  PostalCode      = $PostalCode
  CountryOrRegion = $CountryOrRegion
}
if ($CompanyName -ne "") { $params["CompanyName"] = $CompanyName }
if ($Description -ne "") { $params["Description"] = $Description }

New-CsOnlineLisCivicAddress @params
