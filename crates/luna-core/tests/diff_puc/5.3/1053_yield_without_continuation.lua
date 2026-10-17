-- yields from code PUC runs with no continuation, on a coroutine
local out = {}
local co = coroutine.wrap(function()
  -- a message handler
  local ok, e = xpcall(function() error("x") end, function(m)
    local ok2, e2 = pcall(coroutine.yield, 1)
    out[#out + 1] = "handler " .. tostring(ok2) .. " " .. tostring(e2)
    return m
  end)
  out[#out + 1] = "xpcall " .. tostring(ok)
  -- a hook
  local fired = false
  debug.sethook(function()
    if fired then return end
    fired = true
    local ok3, e3 = pcall(coroutine.yield, 2)
    out[#out + 1] = "hook " .. tostring(ok3) .. " " .. tostring(e3)
  end, "", 1)
  local x = 0
  for i = 1, 3 do x = x + i end
  debug.sethook()
  return "done"
end)
print(co())
for _, s in ipairs(out) do print(s) end
