# OMAKURE_SCHEMA_START
# {
#   "Name": "users_set_voicemail_settings",
#   "Description": "Configure voicemail settings",
#   "Tags": ["teams", "users", "voice", "configure"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "User email or UPN",
#       "Description": "User email or UPN"
#     },
#     {
#       "Name": "voicemail_enabled",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Description": "Enable or disable voicemail"
#     },
#     {
#       "Name": "prompt_language",
#       "Type": "string",
#       "Required": false,
#       "Default": "en-US",
#       "Description": "Voicemail prompt language"
#     },
#     {
#       "Name": "call_answer_rule",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["RegularVoicemail", "InternalVoicemail", "VoicemailAndTransferToOperator"],
#       "Description": "Call answer rule for voicemail"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$VoicemailEnabled = ""
$PromptLanguage = "en-US"
$CallAnswerRule = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--voicemail_enabled" { $VoicemailEnabled = $args[++$i] }
    "--prompt_language" { $PromptLanguage = $args[++$i] }
    "--call_answer_rule" { $CallAnswerRule = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-csonlinevoicemailusersettings?view=teams-ps
$params = @{
  Identity       = $Identity
  PromptLanguage = $PromptLanguage
}
if ($VoicemailEnabled -ne "") {
  $VoicemailEnabledBool = if ($VoicemailEnabled -eq "true") { $true } else { $false }
  $params["VoicemailEnabled"] = $VoicemailEnabledBool
}
if ($CallAnswerRule -ne "") { $params["CallAnswerRule"] = $CallAnswerRule }

Set-CsOnlineVoicemailUserSettings @params
