# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_grant_routing_policy",
#   "Description": "Grant voice routing policy to user",
#   "Tags": ["teams", "voice", "direct-routing", "policy", "grant"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email",
#       "Description": "User email"
#     },
#     {
#       "Name": "policy_name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Voice routing policy name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$PolicyName = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--policy_name" { $PolicyName = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($PolicyName -eq "") { Write-Error "--policy_name is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/grant-csonlinevoiceroutingpolicy?view=teams-ps
Grant-CsOnlineVoiceRoutingPolicy -Identity $Identity -PolicyName $PolicyName
