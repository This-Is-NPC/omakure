# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_voice_apps",
#   "Description": "Create voice applications policy",
#   "Tags": ["teams", "policy", "voice", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_aa_greeting_change",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Enabled",
#       "Description": "Allow auto attendant greeting change"
#     },
#     {
#       "Name": "allow_cq_greeting_change",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Enabled",
#       "Description": "Allow call queue greeting change"
#     },
#     {
#       "Name": "allow_cq_music_change",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Enabled",
#       "Description": "Allow call queue music on hold change"
#     },
#     {
#       "Name": "allow_cq_opt_in_out",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Enabled",
#       "Description": "Allow call queue opt in/out"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowAAGreetingChange = "Enabled"
$AllowCQGreetingChange = "Enabled"
$AllowCQMusicChange = "Enabled"
$AllowCQOptInOut = "Enabled"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_aa_greeting_change" { $AllowAAGreetingChange = $args[++$i] }
    "--allow_cq_greeting_change" { $AllowCQGreetingChange = $args[++$i] }
    "--allow_cq_music_change" { $AllowCQMusicChange = $args[++$i] }
    "--allow_cq_opt_in_out" { $AllowCQOptInOut = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsvoiceapplicationspolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowAAGreetingChange -ne "") { $params["AllowAutoAttendantAfterHoursGreetingChange"] = ($AllowAAGreetingChange -eq "Enabled") }
if ($AllowCQGreetingChange -ne "") { $params["AllowCallQueueOverflowSharedVoicemailGreetingChange"] = ($AllowCQGreetingChange -eq "Enabled") }
if ($AllowCQMusicChange -ne "") { $params["AllowCallQueueMusicOnHoldChange"] = ($AllowCQMusicChange -eq "Enabled") }
if ($AllowCQOptInOut -ne "") { $params["AllowCallQueueAgentOptChange"] = ($AllowCQOptInOut -eq "Enabled") }

New-CsTeamsVoiceApplicationsPolicy @params
