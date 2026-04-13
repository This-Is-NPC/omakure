# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_remove_pstn_gateway",
#   "Description": "Remove PSTN gateway (SBC)",
#   "Tags": ["teams", "voice", "direct-routing", "sbc", "remove"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "SBC FQDN",
#       "Description": "SBC fully qualified domain name"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-csonlinepstngateway?view=teams-ps
Remove-CsOnlinePSTNGateway -Identity $Identity
