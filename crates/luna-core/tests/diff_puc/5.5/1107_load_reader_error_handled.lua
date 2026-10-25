-- a reader that returns no string fails load with luaL_error: the message
-- is positioned at load's caller, and the message handler that is running
-- (an xpcall's) runs on it; load returns what the handler made
local function h(m) return "handled: " .. (string.gsub(m, "^[^:]*:%d+: ", "<pos>: ")) end
print(xpcall(function() return load(function() return {} end) end, h))
print(xpcall(function()
  local n = 0
  return load(function() n = n + 1 if n == 1 then return "return " end return true end)
end, h))
print(pcall(load, function() return {} end))
