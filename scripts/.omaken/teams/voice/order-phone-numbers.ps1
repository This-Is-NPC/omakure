# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_order_phone_numbers",
#   "Description": "Order new phone numbers",
#   "Tags": ["teams", "voice", "pstn", "create"],
#   "Fields": [
#     {
#       "Name": "name",
#       "Type": "string",
#       "Required": false,
#       "Description": "Order name"
#     },
#     {
#       "Name": "number_type",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["UserSubscriber", "AutoAttendant", "CallQueue"],
#       "Description": "Number type"
#     },
#     {
#       "Name": "country",
#       "Type": "string",
#       "Required": true,
#       "Default": "US",
#       "Description": "Country code"
#     },
#     {
#       "Name": "area_code",
#       "Type": "string",
#       "Required": true,
#       "Description": "Area code"
#     },
#     {
#       "Name": "quantity",
#       "Type": "string",
#       "Required": true,
#       "Default": "1",
#       "Description": "Number of phone numbers to order"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Order description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Name = ""
$NumberType = ""
$Country = "US"
$AreaCode = ""
$Quantity = "1"
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--name" { $Name = $args[++$i] }
    "--number_type" { $NumberType = $args[++$i] }
    "--country" { $Country = $args[++$i] }
    "--area_code" { $AreaCode = $args[++$i] }
    "--quantity" { $Quantity = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($NumberType -eq "") { Write-Error "--number_type is required"; exit 1 }
if ($Country -eq "") { Write-Error "--country is required"; exit 1 }
if ($AreaCode -eq "") { Write-Error "--area_code is required"; exit 1 }
if ($Quantity -eq "") { Write-Error "--quantity is required"; exit 1 }

if ($Name -eq "") {
  $Name = "Order-$NumberType-$Country-$AreaCode"
}
if ($Description -eq "") {
  $Description = "Order $Quantity $NumberType number(s) for $Country area code $AreaCode"
}

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csonlinetelephonenumberorder?view=teams-ps
$params = @{
  Name       = $Name
  Description = $Description
  NumberType = $NumberType
  Country    = $Country
  AreaCode   = $AreaCode
  Quantity   = [int]$Quantity
}

New-CsOnlineTelephoneNumberOrder @params
