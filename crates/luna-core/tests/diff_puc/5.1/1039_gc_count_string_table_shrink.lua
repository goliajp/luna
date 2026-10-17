-- many short strings made and dropped: the string table shrinks over the
-- collections that follow as PUC's does
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local t = {}
for i = 1, 20000 do t[i] = "k" .. i end
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
local base = count()
t = nil
collectgarbage(); collectgarbage("stop")
print(count() - base)
collectgarbage(); collectgarbage("stop")
print(count() - base)
collectgarbage(); collectgarbage("stop")
print(count() - base)
