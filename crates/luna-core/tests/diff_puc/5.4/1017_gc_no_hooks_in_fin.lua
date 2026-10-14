-- rule: lgc.GCTM.1
-- only the explicit collections below run, whatever the automatic pacing
collectgarbage("stop")
local n, inside = 0, false
local o = setmetatable({}, {__gc = function()
  inside = true
  local x = 1
  x = x + 1
  inside = false
end})
o = nil
local hits = 0
debug.sethook(function() if inside then hits = hits + 1 end end, "l")
collectgarbage()
debug.sethook()
print("hook lines inside __gc", hits)
