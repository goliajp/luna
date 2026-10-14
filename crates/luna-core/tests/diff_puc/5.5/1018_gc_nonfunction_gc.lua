-- rule: lgc.GCTM.6
setmetatable({}, {__gc = true})
collectgarbage()
local called = 0
setmetatable({}, {__gc = setmetatable({}, {__call = function() called = called + 1 end})})
collectgarbage()
print("callable table __gc called", called)
