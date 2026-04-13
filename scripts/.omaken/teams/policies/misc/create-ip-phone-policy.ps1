# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_ip_phone",
#   "Description": "Create IP phone policy",
#   "Tags": ["teams", "policy", "devices", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "sign_in_mode",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["UserSignIn", "CommonAreaPhoneSignIn", "MeetingSignIn"],
#       "Default": "UserSignIn",
#       "Description": "Sign-in mode"
#     },
#     {
#       "Name": "hot_desking_timeout",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Hot desking idle timeout in minutes",
#       "Default": "10",
#       "Description": "Hot desking idle timeout in minutes"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$SignInMode = "UserSignIn"
$HotDeskingTimeout = "10"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--sign_in_mode" { $SignInMode = $args[++$i] }
    "--hot_desking_timeout" { $HotDeskingTimeout = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsipphonepolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($SignInMode -ne "") { $params["SignInMode"] = $SignInMode }
if ($HotDeskingTimeout -ne "") {
  $timeout = New-TimeSpan -Minutes ([int]$HotDeskingTimeout)
  $params["HotDeskingIdleTimeoutInMinutes"] = $timeout
}

New-CsTeamsIPPhonePolicy @params
