# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_create_translation_rule",
#   "Description": "Create Teams translation rule",
#   "Tags": ["teams", "voice", "direct-routing", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Rule Name",
#       "Description": "Translation rule name"
#     },
#     {
#       "Name": "pattern",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Pattern regex",
#       "Description": "Pattern regex"
#     },
#     {
#       "Name": "translation",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Translation pattern",
#       "Description": "Translation pattern"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Pattern = ""
$Translation = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--pattern" { $Pattern = $args[++$i] }
    "--translation" { $Translation = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($Pattern -eq "") { Write-Error "--pattern is required"; exit 1 }
if ($Translation -eq "") { Write-Error "--translation is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamstranslationrule?view=teams-ps
New-CsTeamsTranslationRule -Identity $Identity -Pattern $Pattern -Translation $Translation
