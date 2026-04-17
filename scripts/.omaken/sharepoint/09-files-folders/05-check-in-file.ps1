#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "check_in_file",
#   "Description": "Check in a file.",
#   "Fields": [
#     {
#       "Name": "FileUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-FileUrl",
#       "Prompt": "Server-relative file URL"
#     },
#     {
#       "Name": "CheckinType",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-CheckinType",
#       "Prompt": "Check-in type",
#       "Choices": ["MinorCheckIn", "MajorCheckIn", "OverwriteCheckIn"]
#     },
#     {
#       "Name": "Comment",
#       "Type": "string",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-Comment",
#       "Prompt": "Check-in comment"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$FileUrl,

    [Parameter(Mandatory = $true)]
    [ValidateSet("MinorCheckIn", "MajorCheckIn", "OverwriteCheckIn")]
    [string]$CheckinType,

    [string]$Comment = ""
)

$params = @{
    Url         = $FileUrl
    CheckinType = $CheckinType
}

if ($Comment -ne "") {
    $params["Comment"] = $Comment
}

Set-PnPFileCheckedIn @params
