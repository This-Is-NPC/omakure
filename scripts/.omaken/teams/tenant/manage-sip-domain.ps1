# OMAKURE_SCHEMA_START
# {
#   "Name": "tenant_manage_sip_domain",
#   "Description": "Enable or disable SIP domain",
#   "Tags": ["teams", "tenant", "configure"],
#   "Fields": [
#     {
#       "Name": "domain",
#       "Type": "string",
#       "Required": true,
#       "Description": "SIP domain name"
#     },
#     {
#       "Name": "action",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["Enable", "Disable"],
#       "Description": "Action to perform"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Domain = ""
$Action = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--domain" { $Domain = $args[++$i] }
    "--action" { $Action = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Domain -eq "") { Write-Error "--domain is required"; exit 1 }
if ($Action -eq "") { Write-Error "--action is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/enable-csonlinesipdomain?view=teams-ps
if ($Action -eq "Enable") {
  Enable-CsOnlineSipDomain -Domain $Domain
} elseif ($Action -eq "Disable") {
  Disable-CsOnlineSipDomain -Domain $Domain
} else {
  Write-Error "Invalid action: $Action. Must be Enable or Disable"; exit 1
}
