#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "add_list_webhook",
#   "Description": "Add a webhook subscription to a list.",
#   "Fields": [
#     {
#       "Name": "ListName",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-ListName",
#       "Prompt": "List name"
#     },
#     {
#       "Name": "NotificationUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-NotificationUrl",
#       "Prompt": "Webhook endpoint URL"
#     },
#     {
#       "Name": "ExpirationDays",
#       "Type": "number",
#       "Required": false,
#       "Order": 3,
#       "Arg": "-ExpirationDays",
#       "Prompt": "Days until expiration",
#       "Default": "180"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$ListName,

    [Parameter(Mandatory = $true)]
    [string]$NotificationUrl,

    [int]$ExpirationDays = 180
)

Add-PnPWebhookSubscription -List $ListName -NotificationUrl $NotificationUrl -ExpirationDate (Get-Date).AddDays($ExpirationDays)
