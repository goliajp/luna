-- rule: lgc.GCTM.5
-- only the explicit collections below run, whatever the automatic pacing
collectgarbage("stop")
if warn then warn("@on") end
setmetatable({}, {__gc = true})
collectgarbage()
local called = 0
setmetatable({}, {__gc = setmetatable({}, {__call = function() called = called + 1 end})})
collectgarbage()
print("callable table __gc called", called)
