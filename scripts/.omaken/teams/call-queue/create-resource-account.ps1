# OMAKURE_SCHEMA_START
# {
#   "Name": "cq_create_resource_account",
#   "Description": "Create resource account for call queue or auto attendant",
#   "Tags": ["teams", "call-queue", "auto-attendant", "resource-account", "create"],
#   "Fields": [
#     {
#       "Name": "user_principal_name",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "UPN (e.g. ra-support@contoso.com)",
#       "Description": "User principal name"
#     },
#     {
#       "Name": "display_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Display name"
#     },
#     {
#       "Name": "application_id",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["11cd3e2e-fccb-42ad-ad00-878b93575e07", "ce933385-9390-45d1-9512-c8d228074e07"],
#       "Prompt": "App ID (11cd...=CallQueue, ce93...=AutoAttendant)",
#       "Description": "Application ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$UserPrincipalName = ""
$DisplayName = ""
$ApplicationId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--user_principal_name" { $UserPrincipalName = $args[++$i] }
    "--display_name" { $DisplayName = $args[++$i] }
    "--application_id" { $ApplicationId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($UserPrincipalName -eq "") { Write-Error "--user_principal_name is required"; exit 1 }
if ($DisplayName -eq "") { Write-Error "--display_name is required"; exit 1 }
if ($ApplicationId -eq "") { Write-Error "--application_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlineapplicationinstance?view=teams-ps
New-CsOnlineApplicationInstance -UserPrincipalName $UserPrincipalName -DisplayName $DisplayName -ApplicationId $ApplicationId
