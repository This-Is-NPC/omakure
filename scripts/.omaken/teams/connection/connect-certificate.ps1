# OMAKURE_SCHEMA_START
# {
#   "Name": "connection_connect_certificate",
#   "Description": "Connect to Microsoft Teams using certificate authentication",
#   "Tags": ["teams", "connection", "certificate"],
#   "Fields": [
#     {
#       "Name": "certificate_thumbprint",
#       "Prompt": "Certificate Thumbprint",
#       "Type": "string",
#       "Order": 1,
#       "Required": true
#     },
#     {
#       "Name": "application_id",
#       "Prompt": "Application (Client) ID",
#       "Type": "string",
#       "Order": 2,
#       "Required": true
#     },
#     {
#       "Name": "tenant_id",
#       "Prompt": "Tenant ID",
#       "Type": "string",
#       "Order": 3,
#       "Required": true
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$CertificateThumbprint = ""
$ApplicationId = ""
$TenantId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--certificate_thumbprint" { $CertificateThumbprint = $args[++$i] }
    "--application_id" { $ApplicationId = $args[++$i] }
    "--tenant_id" { $TenantId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

# https://learn.microsoft.com/en-us/microsoftteams/teams-powershell-application-authentication
Connect-MicrosoftTeams -CertificateThumbprint $CertificateThumbprint -ApplicationId $ApplicationId -TenantId $TenantId
