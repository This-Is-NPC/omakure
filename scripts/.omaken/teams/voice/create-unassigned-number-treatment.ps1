# OMAKURE_SCHEMA_START
# {
#   "Name": "voice_create_unassigned_treatment",
#   "Description": "Create unassigned number treatment",
#   "Tags": ["teams", "voice", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Treatment Name",
#       "Description": "Treatment name"
#     },
#     {
#       "Name": "pattern",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Number pattern regex",
#       "Description": "Number pattern regex"
#     },
#     {
#       "Name": "target_type",
#       "Type": "string",
#       "Required": true,
#       "Choices": ["AutoAttendant", "Announcement"],
#       "Description": "Target type"
#     },
#     {
#       "Name": "target",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Target AA ID or announcement ID",
#       "Description": "Target resource ID"
#     },
#     {
#       "Name": "treatment_priority",
#       "Type": "string",
#       "Required": false,
#       "Default": "1",
#       "Description": "Treatment priority"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$Pattern = ""
$TargetType = ""
$Target = ""
$TreatmentPriority = "1"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--pattern" { $Pattern = $args[++$i] }
    "--target_type" { $TargetType = $args[++$i] }
    "--target" { $Target = $args[++$i] }
    "--treatment_priority" { $TreatmentPriority = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }
if ($Pattern -eq "") { Write-Error "--pattern is required"; exit 1 }
if ($TargetType -eq "") { Write-Error "--target_type is required"; exit 1 }
if ($Target -eq "") { Write-Error "--target is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsunassignednumbertreatment?view=teams-ps
$params = @{
  Identity   = $Identity
  Pattern    = $Pattern
  TargetType = $TargetType
  Target     = $Target
}
if ($TreatmentPriority -ne "") { $params["TreatmentPriority"] = [int]$TreatmentPriority }

New-CsTeamsUnassignedNumberTreatment @params
