#Requires -Version 5.1

# Requires: Connect-PnPOnline (see 01-setup/)

# OMAKURE_SCHEMA_START
# {
#   "Name": "migrate_documents",
#   "Description": "Copy documents from a source library to a target library in another site.",
#   "Fields": [
#     {
#       "Name": "SourceSiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 1,
#       "Arg": "-SourceSiteUrl",
#       "Prompt": "URL of the source SharePoint site"
#     },
#     {
#       "Name": "TargetSiteUrl",
#       "Type": "string",
#       "Required": true,
#       "Order": 2,
#       "Arg": "-TargetSiteUrl",
#       "Prompt": "URL of the target SharePoint site"
#     },
#     {
#       "Name": "LibraryName",
#       "Type": "string",
#       "Required": true,
#       "Order": 3,
#       "Arg": "-LibraryName",
#       "Prompt": "Name of the document library to migrate"
#     }
#   ]
# }
# OMAKURE_SCHEMA_END

param(
    [Parameter(Mandatory = $true)]
    [string]$SourceSiteUrl,

    [Parameter(Mandatory = $true)]
    [string]$TargetSiteUrl,

    [Parameter(Mandatory = $true)]
    [string]$LibraryName
)

Connect-PnPOnline -Url $SourceSiteUrl -Interactive
$files = Get-PnPListItem -List $LibraryName | Where-Object { $_.FileSystemObjectType -eq "File" }

Write-Host "Copying $($files.Count) file(s) from '$LibraryName' to $TargetSiteUrl..."

foreach ($file in $files) {
    $serverRelativeUrl = $file["FileRef"]
    Copy-PnPFile -SourceUrl $serverRelativeUrl -TargetUrl "$TargetSiteUrl/$LibraryName" -Force
    Write-Host "Copied: $serverRelativeUrl"
}

Write-Host "Migration complete."
