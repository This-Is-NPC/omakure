# OMAKURE_SCHEMA_START
# {
#   "Name": "cq_create",
#   "Description": "Create call queue",
#   "Tags": ["teams", "call-queue", "create"],
#   "Fields": [
#     {
#       "Name": "name",
#       "Type": "string",
#       "Required": true,
#       "Description": "Call queue name"
#     },
#     {
#       "Name": "language_id",
#       "Type": "string",
#       "Required": true,
#       "Default": "en-US",
#       "Description": "Language ID"
#     },
#     {
#       "Name": "routing_method",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Attendant", "Serial", "RoundRobin", "LongestIdle"],
#       "Default": "Attendant",
#       "Description": "Routing method"
#     },
#     {
#       "Name": "agent_alert_time",
#       "Type": "string",
#       "Required": false,
#       "Default": "30",
#       "Description": "Agent alert time in seconds"
#     },
#     {
#       "Name": "allow_opt_out",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow agents to opt out"
#     },
#     {
#       "Name": "presence_based_routing",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Enable presence-based routing"
#     },
#     {
#       "Name": "conference_mode",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Enable conference mode"
#     },
#     {
#       "Name": "overflow_threshold",
#       "Type": "string",
#       "Required": false,
#       "Default": "50",
#       "Description": "Overflow threshold"
#     },
#     {
#       "Name": "timeout_threshold",
#       "Type": "string",
#       "Required": false,
#       "Default": "1200",
#       "Description": "Timeout threshold in seconds"
#     },
#     {
#       "Name": "service_level_threshold_response_time_in_second",
#       "Type": "string",
#       "Required": false,
#       "Default": "30",
#       "Description": "Service level threshold response time in seconds"
#     },
#     {
#       "Name": "use_default_music_on_hold",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Use default music on hold"
#     },
#     {
#       "Name": "music_on_hold_audio_file_id",
#       "Type": "string",
#       "Required": false,
#       "Description": "Custom music on hold audio file ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Name = ""
$LanguageId = "en-US"
$RoutingMethod = "Attendant"
$AgentAlertTime = "30"
$AllowOptOut = "true"
$PresenceBasedRouting = "true"
$ConferenceMode = "true"
$OverflowThreshold = "50"
$TimeoutThreshold = "1200"
$ServiceLevelThresholdResponseTimeInSecond = "30"
$UseDefaultMusicOnHold = "true"
$MusicOnHoldAudioFileId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--name" { $Name = $args[++$i] }
    "--language_id" { $LanguageId = $args[++$i] }
    "--routing_method" { $RoutingMethod = $args[++$i] }
    "--agent_alert_time" { $AgentAlertTime = $args[++$i] }
    "--allow_opt_out" { $AllowOptOut = $args[++$i] }
    "--presence_based_routing" { $PresenceBasedRouting = $args[++$i] }
    "--conference_mode" { $ConferenceMode = $args[++$i] }
    "--overflow_threshold" { $OverflowThreshold = $args[++$i] }
    "--timeout_threshold" { $TimeoutThreshold = $args[++$i] }
    "--service_level_threshold_response_time_in_second" { $ServiceLevelThresholdResponseTimeInSecond = $args[++$i] }
    "--use_default_music_on_hold" { $UseDefaultMusicOnHold = $args[++$i] }
    "--music_on_hold_audio_file_id" { $MusicOnHoldAudioFileId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Name -eq "") { Write-Error "--name is required"; exit 1 }
if ($UseDefaultMusicOnHold -eq "false" -and $MusicOnHoldAudioFileId -eq "") {
  Write-Error "--music_on_hold_audio_file_id is required when --use_default_music_on_hold is false"
  exit 1
}

# https://learn.microsoft.com/en-us/powershell/module/teams/new-cscallqueue?view=teams-ps
$params = @{
  Name       = $Name
  LanguageId = $LanguageId
}
if ($RoutingMethod -ne "") { $params["RoutingMethod"] = $RoutingMethod }
if ($AgentAlertTime -ne "") { $params["AgentAlertTime"] = [int]$AgentAlertTime }
if ($AllowOptOut -ne "") {
  $params["AllowOptOut"] = if ($AllowOptOut -eq "true") { $true } else { $false }
}
if ($PresenceBasedRouting -ne "") {
  $params["PresenceBasedRouting"] = if ($PresenceBasedRouting -eq "true") { $true } else { $false }
}
if ($ConferenceMode -ne "") {
  $params["ConferenceMode"] = if ($ConferenceMode -eq "true") { $true } else { $false }
}
if ($OverflowThreshold -ne "") { $params["OverflowThreshold"] = [int]$OverflowThreshold }
if ($TimeoutThreshold -ne "") { $params["TimeoutThreshold"] = [int]$TimeoutThreshold }
if ($ServiceLevelThresholdResponseTimeInSecond -ne "") {
  $params["ServiceLevelThresholdResponseTimeInSecond"] = [int]$ServiceLevelThresholdResponseTimeInSecond
}
if ($UseDefaultMusicOnHold -ne "") {
  $params["UseDefaultMusicOnHold"] = if ($UseDefaultMusicOnHold -eq "true") { $true } else { $false }
}
if ($MusicOnHoldAudioFileId -ne "") { $params["MusicOnHoldAudioFileId"] = $MusicOnHoldAudioFileId }

New-CsCallQueue @params
