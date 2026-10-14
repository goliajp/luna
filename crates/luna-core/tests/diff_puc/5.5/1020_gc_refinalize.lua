-- rule: lgc.luaC_checkfinalizer.3
local n = 0
local saved
local mt = {__gc = function(o) n = n + 1; saved = o end}
setmetatable({}, mt)
collectgarbage(); collectgarbage()
print("first", n)
setmetatable(saved, {__gc = function(o) n = n + 10 end})
saved = nil
collectgarbage(); collectgarbage()
print("second", n)
