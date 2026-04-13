# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_check_number_order",
#   "Description": "Check phone number order status",
#   "Tags": ["teams", "voice", "pstn", "list"],
#   "Fields": [
#     {
#       "Name": "order_id",
#       "Type": "string",
#       "Required": true,
#       "Description": "Order ID"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$OrderId = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--order_id" { $OrderId = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($OrderId -eq "") { Write-Error "--order_id is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/get-csonlinetelephonenumberorder?view=teams-ps
Get-CsOnlineTelephoneNumberOrder -OrderId $OrderId
