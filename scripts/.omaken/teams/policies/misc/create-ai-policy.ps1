# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_ai",
#   "Description": "Create AI policy",
#   "Tags": ["teams", "policy", "ai", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "enroll_face",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Disabled",
#       "Description": "Enroll face"
#     },
#     {
#       "Name": "enroll_voice",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Enabled", "Disabled"],
#       "Default": "Disabled",
#       "Description": "Enroll voice"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$EnrollFace = "Disabled"
$EnrollVoice = "Disabled"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--enroll_face" { $EnrollFace = $args[++$i] }
    "--enroll_voice" { $EnrollVoice = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsaipolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($EnrollFace -ne "") { $params["EnrollFace"] = $EnrollFace }
if ($EnrollVoice -ne "") { $params["EnrollVoice"] = $EnrollVoice }

New-CsTeamsAIPolicy @params
