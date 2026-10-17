collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
collectgarbage(); collectgarbage(); collectgarbage("stop")
print(string.format("%.3f", collectgarbage("count")))
local t = {} for i = 1, 1000 do t[i] = {} end
print(string.format("%.3f", collectgarbage("count")))
