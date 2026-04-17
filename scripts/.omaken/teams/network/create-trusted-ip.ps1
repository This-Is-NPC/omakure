# OMAKURE_SCHEMA_START
# {
#   "Name": "network_create_trusted_ip",
#   "Description": "Create trusted IP address",
#   "Tags": ["teams", "network", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "IP Address",
#       "Description": "IP address"
#     },
#     {
#       "Name": "mask_bits",
#       "Type": "string",
#       "Required": true,
#       "Default": "32",
#       "Description": "Mask bits"
#     },
#     {
#       "Name": "description",
#       "Type": "string",
#       "Required": false,
#       "Description": "Description"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$MaskBits = "32"
$Description = ""
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--mask_bits" { $MaskBits = $args[++$i] }
    "--description" { $Description = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-cstenanttrustediipaddress?view=teams-ps
$params = @{
  Identity = $Identity
  MaskBits = [int]$MaskBits
}
if ($Description -ne "") { $params["Description"] = $Description }

New-CsTenantTrustedIPAddress @params
