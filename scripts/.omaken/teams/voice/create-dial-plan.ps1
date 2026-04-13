# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_create_dial_plan",
#   "Description": "Create tenant dial plan",
#   "Tags": ["teams", "voice", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Dial Plan Name",
#       "Description": "Dial plan name"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Dial plan description"
#     },
#     {
#       "Name": "norm_rule_name",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Normalization rule name",
#       "Description": "Normalization rule name"
#     },
#     {
#       "Name": "norm_rule_pattern",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Pattern regex",
#       "Description": "Normalization rule pattern regex"
#     },
#     {
#       "Name": "norm_rule_translation",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Translation pattern",
#       "Description": "Normalization rule translation pattern"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Description = ""
$NormRuleName = ""
$NormRulePattern = ""
$NormRuleTranslation = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    "--norm_rule_name" { $NormRuleName = $args[++$i] }
    "--norm_rule_pattern" { $NormRulePattern = $args[++$i] }
    "--norm_rule_translation" { $NormRuleTranslation = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-cstenantdialplan?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($Description -ne "") { $params["Description"] = $Description }

if ($NormRuleName -ne "" -and $NormRulePattern -ne "" -and $NormRuleTranslation -ne "") {
  $normRule = New-CsVoiceNormalizationRule -InMemory -Name $NormRuleName -Pattern $NormRulePattern -Translation $NormRuleTranslation
  $params["NormalizationRules"] = @($normRule)
}

New-CsTenantDialPlan @params
