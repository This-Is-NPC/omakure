# OMAKURE_SCHEMA_START
# {
#   "Name": "aa_create",
#   "Description": "Create auto attendant",
#   "Tags": ["teams", "auto-attendant", "create"],
#   "Fields": [
#     {
#       "Name": "name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Auto attendant name"
#     },
#     {
#       "Name": "language_id",
#       "Type": "string",
#       "Required": true,
#       "Default": "en-US",
#       "Description": "Language ID"
#     },
#     {
#       "Name": "time_zone",
#       "Type": "string",
#       "Required": true,
#       "Default": "UTC",
#       "Description": "Time zone"
#     },
#     {
#       "Name": "greeting_text",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "TTS greeting message",
#       "Description": "Text-to-speech greeting message"
#     },
#     {
#       "Name": "operator_identity",
#       "Type": "string",
#       "Required": false,
#       "Prompt": "Operator email (optional)",
#       "Description": "Operator email address"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Name = ""
$LanguageId = "en-US"
$TimeZone = "UTC"
$GreetingText = ""
$OperatorIdentity = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--name" { $Name = $args[++$i] }
    "--language_id" { $LanguageId = $args[++$i] }
    "--time_zone" { $TimeZone = $args[++$i] }
    "--greeting_text" { $GreetingText = $args[++$i] }
    "--operator_identity" { $OperatorIdentity = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Name -eq "") { Write-Error "--name is required"; exit 1 }
if ($GreetingText -eq "") { Write-Error "--greeting_text is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csautoattendant?view=teams-ps
$greetingPrompt = New-CsAutoAttendantPrompt -TextToSpeechPrompt $GreetingText
$defaultMenu = New-CsAutoAttendantMenu -Name "Default Menu" -MenuOptions @() -DirectorySearchMethod None
$defaultCallFlow = New-CsAutoAttendantCallFlow -Name "Default Call Flow" -Greetings @($greetingPrompt) -Menu $defaultMenu

$params = @{
  Name            = $Name
  LanguageId      = $LanguageId
  TimeZoneId      = $TimeZone
  DefaultCallFlow = $defaultCallFlow
}

if ($OperatorIdentity -ne "") {
  $resolvedOperator = Get-CsOnlineUser -Identity $OperatorIdentity
  $operatorEntity = New-CsAutoAttendantCallableEntity -Identity $resolvedOperator.ObjectId -Type User
  $params["Operator"] = $operatorEntity
}

New-CsAutoAttendant @params
