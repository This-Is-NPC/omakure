# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_remove_phone_number",
#   "Description": "Remove phone number from user or resource account",
#   "Tags": ["teams", "voice", "pstn", "remove"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Description": "User or resource account email"
#     },
#     {
#       "Name": "phone_number",
#       "Type": "string",
#       "Required": true,
#       "Description": "Phone number to remove"
#     },
#     {
#       "Name": "phone_number_type",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["DirectRouting", "CallingPlan", "OperatorConnect"],
#       "Description": "Phone number type"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$PhoneNumber = ""
$PhoneNumberType = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--phone_number" { $PhoneNumber = $args[++$i] }
    "--phone_number_type" { $PhoneNumberType = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($PhoneNumber -eq "") { Write-Error "--phone_number is required"; exit 1 }
if ($PhoneNumberType -eq "") { Write-Error "--phone_number_type is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/remove-csphonenumberassignment?view=teams-ps
Remove-CsPhoneNumberAssignment -Identity $Identity -PhoneNumber $PhoneNumber -PhoneNumberType $PhoneNumberType
