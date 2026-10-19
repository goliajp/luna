-- a protected call refused at the C level limit catches its own refusal:
-- its caller sees it fail, as `luaD_pcall` is in place before the call it
-- makes is checked
local function deep(n)
  local ok, r = pcall(deep, n + 1)
  if not ok then return "inner " .. n .. " " .. tostring(r) end
  return r
end
print(deep(0))
local function deepx(n)
  local ok, r = xpcall(deepx, function(m) return "h:" .. tostring(m) end, n + 1)
  if not ok then return "inner " .. n .. " " .. tostring(r) end
  return r
end
print(deepx(0))
