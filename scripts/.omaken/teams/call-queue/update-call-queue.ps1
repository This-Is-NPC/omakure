# OMAKURE_SCHEMA_START
# {
#   "Name": "cq_update",
#   "Description": "Update call queue",
#   "Tags": ["teams", "call-queue", "configure"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Call Queue ID",
#       "Description": "Call queue identity"
#     },
#     {
#       "Name": "name",
#       "Type": "string",
#       "Required": false,
#       "Description": "Call queue name"
#     },
#     {
#       "Name": "routing_method",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Attendant", "Serial", "RoundRobin", "LongestIdle"],
#       "Description": "Routing method"
#     },
#     {
#       "Name": "agent_alert_time",
#       "Type": "string",
#       "Required": false,
#       "Description": "Agent alert time in seconds"
#     },
#     {
#       "Name": "overflow_threshold",
#       "Type": "string",
#       "Required": false,
#       "Description": "Overflow threshold"
#     },
#     {
#       "Name": "timeout_threshold",
#       "Type": "string",
#       "Required": false,
#       "Description": "Timeout threshold in seconds"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Name = ""
$RoutingMethod = ""
$AgentAlertTime = ""
$OverflowThreshold = ""
$TimeoutThreshold = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--name" { $Name = $args[++$i] }
    "--routing_method" { $RoutingMethod = $args[++$i] }
    "--agent_alert_time" { $AgentAlertTime = $args[++$i] }
    "--overflow_threshold" { $OverflowThreshold = $args[++$i] }
    "--timeout_threshold" { $TimeoutThreshold = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/set-cscallqueue?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($Name -ne "") { $params["Name"] = $Name }
if ($RoutingMethod -ne "") { $params["RoutingMethod"] = $RoutingMethod }
if ($AgentAlertTime -ne "") { $params["AgentAlertTime"] = [int]$AgentAlertTime }
if ($OverflowThreshold -ne "") { $params["OverflowThreshold"] = [int]$OverflowThreshold }
if ($TimeoutThreshold -ne "") { $params["TimeoutThreshold"] = [int]$TimeoutThreshold }

Set-CsCallQueue @params
