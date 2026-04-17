#Requires -Version 5.1
# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "set_access_request_emails",
#   "Description": "Set the email addresses for site access requests.",
#   "Fields": [
#     { "Name": "Emails", "Type": "string", "Required": true, "Order": 1, "Arg": "-Emails", "Description": "Comma-separated email addresses" }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory=$true)]
    [string]$Emails
)

$emailList = $Emails -split ","
Set-PnPRequestAccessEmails -Emails $emailList
