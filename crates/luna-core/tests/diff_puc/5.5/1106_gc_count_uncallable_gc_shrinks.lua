-- a __gc that cannot be called fails in GCTM's protected call, which
-- shrinks the stack: it frees half of the unused call records the
-- collection's own shrink left
collectgarbage("stop")
local function deep(n) if n > 0 then return deep(n - 1) + 1 end return 0 end
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
deep(300)
local base = count()
setmetatable({}, {__gc = true})
collectgarbage()
print(count() - base)
deep(300)
base = count()
setmetatable({}, {__gc = 42})
setmetatable({}, {__gc = "x"})
collectgarbage()
print(count() - base)
