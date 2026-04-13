# OMAKURE_SCHEMA_START
# {
#   "Name": "connection_connect_access_token",
#   "Description": "Connect to Microsoft Teams using access token authentication",
#   "Tags": ["teams", "connection", "token"],
#   "Fields": [
#     {
#       "Name": "application_id",
#       "Prompt": "Application (Client) ID",
#       "Type": "string",
#       "Order": 1,
#       "Required": true
#     },
#     {
#       "Name": "tenant_id",
#       "Prompt": "Tenant ID",
#       "Type": "string",
#       "Order": 2,
#       "Required": true
#     },
#     {
#       "Name": "client_secret",
#       "Prompt": "Client Secret",
#       "Type": "string",
#       "Order": 3,
#       "Required": true
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$ApplicationId = ""
$TenantId = ""
$ClientSecret = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--application_id" { $ApplicationId = $args[++$i] }
    "--tenant_id" { $TenantId = $args[++$i] }
    "--client_secret" { $ClientSecret = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

# https://learn.microsoft.com/en-us/microsoftteams/teams-powershell-application-authentication

# Request Graph API token
$GraphTokenBody = @{
  Grant_Type    = "client_credentials"
  Scope         = "https://graph.microsoft.com/.default"
  Client_Id     = $ApplicationId
  Client_Secret = $ClientSecret
}
$GraphTokenResponse = Invoke-RestMethod -Uri "https://login.microsoftonline.com/$TenantId/oauth2/v2.0/token" -Method POST -Body $GraphTokenBody
$GraphAccessToken = $GraphTokenResponse.access_token

# Request Teams API token
$TeamsTokenBody = @{
  Grant_Type    = "client_credentials"
  Scope         = "48ac35b8-9aa8-4d74-927d-1f4a14a0b239/.default"
  Client_Id     = $ApplicationId
  Client_Secret = $ClientSecret
}
$TeamsTokenResponse = Invoke-RestMethod -Uri "https://login.microsoftonline.com/$TenantId/oauth2/v2.0/token" -Method POST -Body $TeamsTokenBody
$TeamsAccessToken = $TeamsTokenResponse.access_token

Connect-MicrosoftTeams -AccessTokens @($GraphAccessToken, $TeamsAccessToken)
