# OMAKURE_SCHEMA_START
# {
#   "Name": "connection_connect_interactive",
#   "Description": "Connect to Microsoft Teams using interactive browser login",
#   "Tags": ["teams", "connection"],
#   "Fields": []
# }
# OMAKURE_SCHEMA_END

$Username = ""
$Password = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--username" { $Username = $args[++$i] }
    "--password" { $Password = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

# https://learn.microsoft.com/en-us/powershell/module/teams/connect-microsoftteams?view=teams-ps
if ($Username -ne "" -and $Password -ne "") {
  $SecurePassword = ConvertTo-SecureString $Password -AsPlainText -Force
  $Credential = New-Object System.Management.Automation.PSCredential($Username, $SecurePassword)
  Connect-MicrosoftTeams -Credential $Credential
} else {
  Connect-MicrosoftTeams
}
