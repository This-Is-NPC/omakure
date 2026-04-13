# OMAKURE_SCHEMA_START
# {
#   "Name": "cq_associate_resource_account",
#   "Description": "Associate resource account with auto attendant or call queue",
#   "Tags": ["teams", "call-queue", "auto-attendant", "resource-account", "configure"],
#   "Fields": [
#     {
#       "Name": "resource_account",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Resource account UPN",
#       "Description": "Resource account user principal name"
#     },
#     {
#       "Name": "configuration_id",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "AA or CQ ID",
#       "Description": "Auto attendant or call queue ID"
#     },
#     {
#       "Name": "configuration_type",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["AutoAttendant", "CallQueue"],
#       "Description": "Configuration type"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$ResourceAccount = ""
$ConfigurationId = ""
$ConfigurationType = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--resource_account" { $ResourceAccount = $args[++$i] }
    "--configuration_id" { $ConfigurationId = $args[++$i] }
    "--configuration_type" { $ConfigurationType = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($ResourceAccount -eq "") { Write-Error "--resource_account is required"; exit 1 }
if ($ConfigurationId -eq "") { Write-Error "--configuration_id is required"; exit 1 }
if ($ConfigurationType -eq "") { Write-Error "--configuration_type is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlineapplicationinstanceassociation?view=teams-ps
$resourceAccountUser = Get-CsOnlineUser -Identity $ResourceAccount
New-CsOnlineApplicationInstanceAssociation -Identities @($resourceAccountUser.ObjectId) -ConfigurationId $ConfigurationId -ConfigurationType $ConfigurationType
