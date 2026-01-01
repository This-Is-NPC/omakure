-- Omakure Azure widget (hello world example).
-- This widget prints the logged-in user and active subscription via Azure CLI.

local function run(cmd)
  local pipe = io.popen(cmd)
  if not pipe then
    return nil
  end
  local out = pipe:read("*a") or ""
  pipe:close()
  return out
end

local function split_tsv(line)
  local parts = {}
  for part in string.gmatch(line, "[^\t\r\n]+") do
    table.insert(parts, part)
  end
  return parts
end

local title = "Azure"

local output = run('az account show --only-show-errors --query "[user.name, name, id]" -o tsv')
if not output or output:gsub("%s+", "") == "" then
  return {
    title = title,
    lines = {
      "Azure CLI not ready. Run `az login`."
    }
  }
end

local parts = split_tsv(output)
local user = parts[1] or "<unknown>"
local sub_name = parts[2] or "<unknown>"
local sub_id = parts[3] or "<unknown>"

return {
  title = title,
  lines = {
    "User: " .. user,
    "Subscription: " .. sub_name .. " (" .. sub_id .. ")"
  }
}
