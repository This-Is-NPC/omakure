# OMAKURE_SCHEMA_START
# {
#   "Name": "policy_create_messaging",
#   "Description": "Create messaging policy",
#   "Tags": ["teams", "policy", "messaging", "create"],
#   "Fields": [
#     {
#       "Name": "identity",
#       "Type": "string",
#       "Required": true,
#       "Prompt": "Policy Name",
#       "Description": "Policy name"
#     },
#     {
#       "Name": "allow_giphy",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow Giphy"
#     },
#     {
#       "Name": "giphy_rating",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["Strict", "Moderate", "NoRestriction"],
#       "Default": "Moderate",
#       "Description": "Giphy content rating"
#     },
#     {
#       "Name": "allow_memes",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow memes"
#     },
#     {
#       "Name": "allow_stickers",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow stickers"
#     },
#     {
#       "Name": "allow_user_edit_message",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow user to edit messages"
#     },
#     {
#       "Name": "allow_user_delete_message",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow user to delete messages"
#     },
#     {
#       "Name": "allow_user_chat",
#       "Type": "string",
#       "Required": false,
#       "Choices": ["true", "false"],
#       "Default": "true",
#       "Description": "Allow user chat"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

$Identity = ""
$AllowGiphy = "true"
$GiphyRating = "Moderate"
$AllowMemes = "true"
$AllowStickers = "true"
$AllowUserEditMessage = "true"
$AllowUserDeleteMessage = "true"
$AllowUserChat = "true"
for ($i = 0; $i -lt $args.Length; $i++) {
  switch ($args[$i]) {
    "--identity" { $Identity = $args[++$i] }
    "--allow_giphy" { $AllowGiphy = $args[++$i] }
    "--giphy_rating" { $GiphyRating = $args[++$i] }
    "--allow_memes" { $AllowMemes = $args[++$i] }
    "--allow_stickers" { $AllowStickers = $args[++$i] }
    "--allow_user_edit_message" { $AllowUserEditMessage = $args[++$i] }
    "--allow_user_delete_message" { $AllowUserDeleteMessage = $args[++$i] }
    "--allow_user_chat" { $AllowUserChat = $args[++$i] }
    default { Write-Error "Unknown arg: $($args[$i])"; exit 1 }
  }
}

if ($Identity -eq "") { Write-Error "--identity is required"; exit 1 }

# https://learn.microsoft.com/en-us/powershell/module/teams/new-csteamsmessagingpolicy?view=teams-ps
$params = @{
  Identity = $Identity
}
if ($AllowGiphy -ne "") {
  $params["AllowGiphy"] = if ($AllowGiphy -eq "true") { $true } else { $false }
}
if ($GiphyRating -ne "") { $params["GiphyRatingType"] = $GiphyRating }
if ($AllowMemes -ne "") {
  $params["AllowMemes"] = if ($AllowMemes -eq "true") { $true } else { $false }
}
if ($AllowStickers -ne "") {
  $params["AllowStickers"] = if ($AllowStickers -eq "true") { $true } else { $false }
}
if ($AllowUserEditMessage -ne "") {
  $params["AllowUserEditMessage"] = if ($AllowUserEditMessage -eq "true") { $true } else { $false }
}
if ($AllowUserDeleteMessage -ne "") {
  $params["AllowUserDeleteMessage"] = if ($AllowUserDeleteMessage -eq "true") { $true } else { $false }
}
if ($AllowUserChat -ne "") {
  $params["AllowUserChat"] = if ($AllowUserChat -eq "true") { $true } else { $false }
}

New-CsTeamsMessagingPolicy @params
