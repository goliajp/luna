-- the default package.path copied out before the field gets another
-- value: the copy keeps the default string until it is dropped
collectgarbage("stop")
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
collectgarbage()
collectgarbage()
local base = count()
local old = package.path
package.path = "x;" .. old
collectgarbage()
print("copy held", count() - base)
package.path = "x"
collectgarbage()
print("concatenation dropped", count() - base)
old = nil
collectgarbage()
print("copy dropped", count() - base)
