param(
  [string]$Repo = $env:REPO,
  [string]$Version = $env:VERSION,
  [string]$BinDir = $env:BIN_DIR,
  [switch]$InstallNodeService,
  [switch]$UninstallNodeService,
  [string]$NodeTokensFile = $env:NODE_TOKENS_FILE,
  [switch]$UninstallNodeState,
  [switch]$Confirmed
)

function Assert-Administrator {
  $identity = [Security.Principal.WindowsIdentity]::GetCurrent()
  $principal = New-Object Security.Principal.WindowsPrincipal($identity)
  if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {
    throw "Machine node-service provisioning requires an elevated Administrator shell; normal installs remain per-user."
  }
}

function Write-NodeConfigIfMissing {
  param([string]$ConfigPath)
  $parent = Split-Path $ConfigPath -Parent
  New-Item -ItemType Directory -Force -Path $parent | Out-Null
  if (-not (Test-Path $ConfigPath)) {
    @"
version = 1

[node]
display_name = ""

[api]
bind = "127.0.0.1:7878"

[network]
mode = "direct"
relays = []
static_peers = []
max_message_bytes = 1048576

[trust]
enrollment = "disabled"
allow_remote_cues = false
allow_baseline_push = false

[discovery]
enabled = false
port = 38383
multicast_addr = "239.255.42.99"
broadcast = true

[organization]
id = ""
discovery_secret_ref = ""
"@ | Set-Content -Path $ConfigPath -Encoding UTF8 -NoNewline
  }
}

function Install-NodeTokens {
  param([string]$Destination)
  if (-not $NodeTokensFile -or -not (Test-Path $NodeTokensFile -PathType Leaf)) {
    throw "-NodeTokensFile must name an existing hashed tokens TOML."
  }
  Copy-Item -Path $NodeTokensFile -Destination $Destination -Force
}

function Prepare-NodeAclAccess {
  param([string]$Path)
  if (Test-Path $Path) {
    & takeown.exe /F $Path /R /D Y | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "could not take temporary ownership of node state" }
    & icacls.exe $Path /grant:r "BUILTIN\Administrators:(OI)(CI)F" /T /C | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "could not prepare elevated access to node state" }
  }
}

function Set-ExactNodeAcl {
  param(
    [string]$Path,
    [bool]$Directory,
    [bool]$ServiceModify
  )
  if (-not (Test-Path $Path)) { return }
  $security = Get-Acl -Path $Path
  $security.SetAccessRuleProtection($true, $false)
  foreach ($rule in @($security.Access)) {
    [void]$security.RemoveAccessRule($rule)
  }
  $security.SetOwner([System.Security.Principal.NTAccount]::new("NT AUTHORITY\SYSTEM"))
  $inheritance = if ($Directory) {
    [System.Security.AccessControl.InheritanceFlags]::ContainerInherit -bor
      [System.Security.AccessControl.InheritanceFlags]::ObjectInherit
  } else {
    [System.Security.AccessControl.InheritanceFlags]::None
  }
  $propagation = [System.Security.AccessControl.PropagationFlags]::None
  $serviceRights = if ($ServiceModify) {
    [System.Security.AccessControl.FileSystemRights]::Modify
  } else {
    [System.Security.AccessControl.FileSystemRights]::Read
  }
  $allow = [System.Security.AccessControl.AccessControlType]::Allow
  $systemRule = [System.Security.AccessControl.FileSystemAccessRule]::new(
    "NT AUTHORITY\SYSTEM",
    [System.Security.AccessControl.FileSystemRights]::FullControl,
    $inheritance,
    $propagation,
    $allow
  )
  $serviceRule = [System.Security.AccessControl.FileSystemAccessRule]::new(
    "NT AUTHORITY\LocalService",
    $serviceRights,
    $inheritance,
    $propagation,
    $allow
  )
  [void]$security.AddAccessRule($systemRule)
  [void]$security.AddAccessRule($serviceRule)
  Set-Acl -Path $Path -AclObject $security
}

function Restore-NodeAcls {
  param(
    [string]$Root,
    [string]$Workspace,
    [string]$Config,
    [string]$Tokens
  )
  if (Test-Path $Root) {
    Get-ChildItem -Path $Root -File -Recurse -Force -ErrorAction SilentlyContinue |
      ForEach-Object { Set-ExactNodeAcl $_.FullName $false $false }
    Set-ExactNodeAcl $Root $true $true
  }
  Set-ExactNodeAcl $Workspace $true $true
  Set-ExactNodeAcl $Config $false $false
  Set-ExactNodeAcl $Tokens $false $false
}

function Install-NodeService {
  param([string]$BinaryPath)
  Assert-Administrator
  $root = Join-Path $env:ProgramData "Omakure"
  $state = $root
  $workspace = Join-Path $env:ProgramData "Omakure-Workspace"
  $config = Join-Path $root "node.toml"
  $tokens = Join-Path $root "tokens.toml"
  Prepare-NodeAclAccess $root
  try {
    New-Item -ItemType Directory -Force -Path $state, $workspace | Out-Null
    Write-NodeConfigIfMissing $config
    Install-NodeTokens $tokens
    $binPath = "`"$BinaryPath`" --scripts-dir `"$workspace`" node serve --tokens-file `"$tokens`""
    & sc.exe query OmakureNode | Out-Null
    $serviceExists = ($LASTEXITCODE -eq 0)
    if ($serviceExists) {
      & sc.exe config OmakureNode binPath= $binPath obj= "NT AUTHORITY\LocalService" start= auto DisplayName= "Omakure Machine Node Service" | Out-Null
    } else {
      & sc.exe create OmakureNode binPath= $binPath obj= "NT AUTHORITY\LocalService" start= auto DisplayName= "Omakure Machine Node Service" | Out-Null
    }
    if ($LASTEXITCODE -ne 0) { throw "sc.exe could not register OmakureNode" }
    & sc.exe failure OmakureNode reset= 86400 actions= restart/5000/restart/5000/none | Out-Null
  } finally {
    Restore-NodeAcls $root $workspace $config $tokens
  }
  Write-Output "Provisioned Windows service OmakureNode; node state/configuration were preserved."
}

function Uninstall-NodeService {
  Assert-Administrator
  & sc.exe stop OmakureNode | Out-Null
  & sc.exe delete OmakureNode | Out-Null
  if ($UninstallNodeState) {
    if (-not $Confirmed) { throw "-UninstallNodeState requires -Confirmed." }
    $root = Join-Path $env:ProgramData "Omakure"
    Remove-Item -Path $root -Recurse -Force -ErrorAction SilentlyContinue
    Remove-Item -Path (Join-Path $env:ProgramData "Omakure-Workspace") -Recurse -Force -ErrorAction SilentlyContinue
  }
  Write-Output "Removed Windows service OmakureNode; node state was $(if ($UninstallNodeState) { 'removed' } else { 'preserved' })."
}

if ($InstallNodeService -and $UninstallNodeService) { throw "-InstallNodeService and -UninstallNodeService cannot be combined." }
if ($UninstallNodeState -and -not $UninstallNodeService) { throw "-UninstallNodeState requires -UninstallNodeService." }
if ($InstallNodeService -and (-not $NodeTokensFile -or -not (Test-Path $NodeTokensFile -PathType Leaf))) {
  throw "-InstallNodeService requires an existing -NodeTokensFile with hashed entries."
}
if ($UninstallNodeService) {
  Uninstall-NodeService
  exit 0
}

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

$processorArchitecture = if (-not [string]::IsNullOrWhiteSpace($env:PROCESSOR_ARCHITEW6432)) {
  $env:PROCESSOR_ARCHITEW6432
} else {
  $env:PROCESSOR_ARCHITECTURE
}
$arch = switch ($processorArchitecture.ToUpperInvariant()) {
  "ARM64" { "aarch64"; break }
  "AMD64" { "x86_64"; break }
  "X86" { "x86_64"; break }
  default { throw "Unsupported architecture: $processorArchitecture" }
}
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

if ($InstallNodeService) {
  Install-NodeService -BinaryPath (Join-Path $BinDir "omakure.exe")
}

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
