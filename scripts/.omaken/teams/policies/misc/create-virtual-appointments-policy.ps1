# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_virtual_appointments",
#   "Description": "Create virtual appointments policy",
#   "Tags": ["teams", "policy", "virtual-appointments", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "Policy name"
#     },
#     {
#       "Name": "enable_sms_notifications",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Enable SMS notifications"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$EnableSmsNotifications = "true"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--enable_sms_notifications" { $EnableSmsNotifications = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsvirtualappointmentspolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($EnableSmsNotifications -ne "") {
  $params["EnableSmsNotifications"] = if ($EnableSmsNotifications -eq "true") { $true } else { $false }
}

New-CsTeamsVirtualAppointmentsPolicy @params
