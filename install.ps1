param(
  [string]$Repo = $env:REPO,
  [string]$Version = $env:VERSION,
  [string]$BinDir = $env:BIN_DIR
)

function Copy-RepoScripts {
  param(
    [string]$Repo,
    [string]$Version,
    [string]$ScriptsDir,
    [string]$TempDir
  )

  $sourceUrl = "https://github.com/$Repo/archive/refs/tags/$Version.zip"
  $sourceZip = Join-Path $TempDir "omakure-$Version-src.zip"
  $sourceDir = Join-Path $TempDir "omakure-src"

  try {
    Invoke-WebRequest -Uri $sourceUrl -OutFile $sourceZip -ErrorAction Stop
    if (Test-Path $sourceDir) {
      Remove-Item -Path $sourceDir -Recurse -Force
    }
    Expand-Archive -Path $sourceZip -DestinationPath $sourceDir -Force
  } catch {
    Write-Warning "Failed to download scripts from ${sourceUrl}: $($_.Exception.Message)"
    return
  }

  $scriptsRoot = Get-ChildItem -Path $sourceDir -Directory -Recurse -Filter "scripts" | Select-Object -First 1
  if (-not $scriptsRoot) {
    Write-Warning "Scripts folder not found in source archive."
    return
  }

  $copied = 0
  $skipped = 0
  Get-ChildItem -Path $scriptsRoot.FullName -File -Recurse | ForEach-Object {
    $relative = $_.FullName.Substring($scriptsRoot.FullName.Length).TrimStart('\', '/')
    $dest = Join-Path $ScriptsDir $relative
    if (Test-Path $dest) {
      $skipped++
      return
    }
    $destParent = Split-Path $dest -Parent
    if (-not (Test-Path $destParent)) {
      New-Item -ItemType Directory -Force -Path $destParent | Out-Null
    }
    Copy-Item -Path $_.FullName -Destination $dest
    $copied++
  }

  if ($copied -gt 0) {
    Write-Output "Copied $copied script(s) to $ScriptsDir"
  } elseif ($skipped -gt 0) {
    Write-Output "Scripts already up to date in $ScriptsDir"
  }
}

if (-not $Repo) {
  $Repo = "This-Is-NPC/omakure"
}

if (-not $Repo) {
  Write-Error "Missing REPO value."
  exit 1
}

if (-not $Version) {
  $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repo/releases/latest"
  $Version = $release.tag_name
}

if (-not $Version) {
  Write-Error "Failed to resolve release version"
  exit 1
}

$arch = if ($env:PROCESSOR_ARCHITECTURE -eq "ARM64") { "aarch64" } else { "x86_64" }
$asset = "omakure-$Version-windows-$arch.zip"
$url = "https://github.com/$Repo/releases/download/$Version/$asset"

$tempDir = Join-Path $env:TEMP "omakure-install"
New-Item -ItemType Directory -Force -Path $tempDir | Out-Null
$zipPath = Join-Path $tempDir $asset

Invoke-WebRequest -Uri $url -OutFile $zipPath
Expand-Archive -Path $zipPath -DestinationPath $tempDir -Force

$exe = Join-Path $tempDir "omakure.exe"
if (-not (Test-Path $exe)) {
  $exe = Get-ChildItem -Path $tempDir -Recurse -Filter "omakure.exe" | Select-Object -First 1 | ForEach-Object { $_.FullName }
}

if (-not $exe) {
  Write-Error "omakure.exe not found in archive"
  exit 1
}

if (-not $BinDir) {
  $BinDir = Join-Path $env:LOCALAPPDATA "omakure\\bin"
}

$documents = [Environment]::GetFolderPath("MyDocuments")
if (-not $documents) { $documents = Join-Path $env:USERPROFILE "Documents" }
$scriptsDir = Join-Path $documents "omakure-scripts"
$legacyScriptsDirs = @(
  (Join-Path $documents "overture-scripts"),
  (Join-Path $documents "cloud-mgmt-scripts")
)
foreach ($legacyDir in $legacyScriptsDirs) {
  if (-not (Test-Path $scriptsDir) -and (Test-Path $legacyDir)) {
    $scriptsDir = $legacyDir
    break
  }
}
New-Item -ItemType Directory -Force -Path $scriptsDir | Out-Null

New-Item -ItemType Directory -Force -Path $BinDir | Out-Null
Copy-Item -Path $exe -Destination (Join-Path $BinDir "omakure.exe") -Force

Copy-RepoScripts -Repo $Repo -Version $Version -ScriptsDir $scriptsDir -TempDir $tempDir

$envKey = "HKCU:\\Environment"
$pathValue = (Get-ItemProperty -Path $envKey -Name Path -ErrorAction SilentlyContinue).Path
if (-not $pathValue) { $pathValue = "" }
$escaped = [Regex]::Escape($BinDir)

if ($pathValue -notmatch $escaped) {
  if ($pathValue -ne "") {
    $newValue = "$pathValue;$BinDir"
  } else {
    $newValue = $BinDir
  }
  Set-ItemProperty -Path $envKey -Name Path -Value $newValue
  Write-Output "Added to PATH: $BinDir"
} else {
  Write-Output "PATH already contains: $BinDir"
}

Write-Output "Installed omakure $Version to $BinDir\\omakure.exe"
Write-Output "Scripts folder: $scriptsDir"
Write-Output "Open a new terminal and run 'omakure'."
