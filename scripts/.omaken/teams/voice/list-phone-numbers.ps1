# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_list_phone_numbers",
#   "Description": "List phone numbers",
#   "Tags": ["teams", "voice", "pstn", "list"],
#   "Fields": [
#     {
#       "Name": "number_type",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["CallingPlan", "DirectRouting", "OperatorConnect"],
#       "Description": "Phone number type filter"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$NumberType = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--number_type" { $NumberType = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

# https://learn.microsoft.com/en-us/powershell/module/teams/get-csphonenumberassignment?view=teams-ps
$params = @{}
if ($NumberType -ne "") { $params["NumberType"] = $NumberType }

Get-CsPhoneNumberAssignment @params
