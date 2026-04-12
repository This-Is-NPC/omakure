# OMAKURE_SCHEMA_START
# {
#   "Name": "shifts_create_connection",
#   "Description": "Create shifts connection to WFM provider",
#   "Tags": ["teams", "shifts", "create"],
#   "Fields": [
#     {
#       "Name": "name",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Connection Name",
#       "Description": "Connection name"
#     },
#     {
#       "Name": "connector_id",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Connector ID",
#       "Description": "Connector ID"
#     },
#     {
#       "Name": "state",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Active", "Disabled"],
#       "Default": "Active",
#       "Description": "Connection state"
#     },
#     {
#       "Name": "connector_specific_settings_json",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Connector settings JSON",
#       "Description": "JSON object matching the selected connector settings schema"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Name = ""
$ConnectorId = ""
$State = "Active"
$ConnectorSpecificSettingsJson = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--name" { $Name = $args[++$i] }
    "--connector_id" { $ConnectorId = $args[++$i] }
    "--state" { $State = $args[++$i] }
    "--connector_specific_settings_json" { $ConnectorSpecificSettingsJson = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Name -eq "") { Write-Error "--name is required"; exit 1 }
if ($ConnectorId -eq "") { Write-Error "--connector_id is required"; exit 1 }
if ($ConnectorSpecificSettingsJson -eq "") { Write-Error "--connector_specific_settings_json is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsshiftsconnection?view=teams-ps
$ConnectorSpecificSettings = ConvertFrom-Json -InputObject $ConnectorSpecificSettingsJson -AsHashtable
New-CsTeamsShiftsConnection -Name $Name -ConnectorId $ConnectorId -State $State -ConnectorSpecificSettings $ConnectorSpecificSettings
