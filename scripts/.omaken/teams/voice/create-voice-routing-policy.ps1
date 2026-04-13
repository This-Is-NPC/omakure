# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_create_routing_policy",
#   "Description": "Create online voice routing policy",
#   "Tags": ["teams", "voice", "direct-routing", "policy", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Policy Name",
#       "Description": "Voice routing policy name"
#     },
#     {
#       "Name": "online_pstn_usages",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "PSTN usages (comma-separated)",
#       "Description": "PSTN usages comma-separated"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Policy description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$OnlinePstnUsages = ""
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--online_pstn_usages" { $OnlinePstnUsages = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($OnlinePstnUsages -eq "") { Write-Error "--online_pstn_usages is required"; exit 1 }

$UsagesArray = $OnlinePstnUsages -split ","

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlinevoiceroutingpolicy?view=teams-ps
$params = @{
  Identity         = $Identity
  OnlinePstnUsages = @($UsagesArray)
}
if ($Description -ne "") { $params["Description"] = $Description }

New-CsOnlineVoiceRoutingPolicy @params
