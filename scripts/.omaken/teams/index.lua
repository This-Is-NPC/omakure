local function trim(value)
  if not value then
    return ""
  end
  return (value:gsub("^%s+", ""):gsub("%s+$", ""))
end

local function is_windows()
  return package.config:sub(1, 1) == "\\"
end

local function powershell_program()
  if is_windows() then
    return "powershell"
  end
  return "pwsh"
end

local function shell_stderr_redirect()
  if is_windows() then
    return "2>NUL"
  end
  return "2>/dev/null"
end

local function run_status(cmd)
  local pipe = io.popen(cmd)
  if not pipe then
    return "", false
  end
  local out = pipe:read("*a") or ""
  local ok, _, status = pipe:close()
  local success = ok == true or status == 0
  return out, success
end

local function parse_pairs(output)
  local pairs = {}
  for line in string.gmatch(output or "", "[^\r\n]+") do
    local key, value = line:match("^([^\t]+)\t(.*)$")
    if key then
      pairs[trim(key)] = trim(value)
    end
  end
  return pairs
end

local function value_or_unknown(value)
  local trimmed = trim(value)
  if trimmed == "" then
    return "<unknown>"
  end
  return trimmed
end

local function base64_encode(data)
  local alphabet = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
  local result = {}
  local remainder = #data % 3

  for i = 1, #data, 3 do
    local a = data:byte(i) or 0
    local b = data:byte(i + 1) or 0
    local c = data:byte(i + 2) or 0
    local value = a * 65536 + b * 256 + c

    local c1 = math.floor(value / 262144) % 64 + 1
    local c2 = math.floor(value / 4096) % 64 + 1
    local c3 = math.floor(value / 64) % 64 + 1
    local c4 = value % 64 + 1

    result[#result + 1] = alphabet:sub(c1, c1)
    result[#result + 1] = alphabet:sub(c2, c2)
    result[#result + 1] = alphabet:sub(c3, c3)
    result[#result + 1] = alphabet:sub(c4, c4)
  end

  if remainder > 0 then
    result[#result] = "="
    if remainder == 1 then
      result[#result - 1] = "="
    end
  end

  return table.concat(result)
end

local function utf16le_base64(value)
  local utf16 = {}
  for i = 1, #value do
    utf16[#utf16 + 1] = string.char(value:byte(i))
    utf16[#utf16 + 1] = "\0"
  end
  return base64_encode(table.concat(utf16))
end

local title = "Teams"

local ps_script = [[
$ErrorActionPreference = "Stop"

function Write-Line([string]$Key, $Value) {
  if ($null -eq $Value) {
    $Value = ""
  }
  Write-Output ($Key + "`t" + ([string]$Value))
}

function Get-CountValue([scriptblock]$Block) {
  try {
    return @(& $Block).Count
  } catch {
    return "<unavailable>"
  }
}

if (-not (Get-Module -ListAvailable -Name MicrosoftTeams)) {
  Write-Line "status" "missing_module"
  exit 0
}

try {
  $tenant = Get-CsTenant
} catch {
  Write-Line "status" "not_ready"
  exit 0
}

$tenantName = $tenant.DisplayName
if ([string]::IsNullOrWhiteSpace([string]$tenantName)) {
  $tenantName = $tenant.Domain
}
if ([string]::IsNullOrWhiteSpace([string]$tenantName)) {
  $tenantName = $tenant.Name
}

$tenantId = $tenant.TenantId
if ([string]::IsNullOrWhiteSpace([string]$tenantId)) {
  $tenantId = $tenant.ObjectId
}

Write-Line "status" "connected"
Write-Line "tenant_name" $tenantName
Write-Line "tenant_id" $tenantId
Write-Line "teams_count" (Get-CountValue { Get-Team })
Write-Line "call_queue_count" (Get-CountValue { Get-CsCallQueue })
Write-Line "auto_attendant_count" (Get-CountValue { Get-CsAutoAttendant })
Write-Line "phone_number_count" (Get-CountValue { Get-CsPhoneNumberAssignment })
]]

local cmd = powershell_program()
  .. " -NoProfile -EncodedCommand "
  .. utf16le_base64(ps_script)
  .. " "
  .. shell_stderr_redirect()

local output, ok = run_status(cmd)
if not ok then
  return {
    title = title,
    lines = {
      "Teams PowerShell not ready.",
      "Use the connection scripts to sign in."
    }
  }
end

local data = parse_pairs(output)
local status = data.status or ""

if status == "missing_module" then
  return {
    title = title,
    lines = {
      "MicrosoftTeams module not installed.",
      "Install it before using Teams scripts."
    }
  }
end

if status ~= "connected" then
  return {
    title = title,
    lines = {
      "Teams PowerShell not ready.",
      "Use the connection scripts to sign in."
    }
  }
end

return {
  title = title,
  lines = {
    "Session: connected",
    "Tenant: " .. value_or_unknown(data.tenant_name),
    "Tenant ID: " .. value_or_unknown(data.tenant_id),
    "Teams: " .. value_or_unknown(data.teams_count),
    "Call queues: " .. value_or_unknown(data.call_queue_count),
    "Auto attendants: " .. value_or_unknown(data.auto_attendant_count),
    "Phone numbers: " .. value_or_unknown(data.phone_number_count)
  }
}
