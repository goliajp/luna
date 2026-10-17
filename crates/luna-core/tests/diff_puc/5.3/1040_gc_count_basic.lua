-- a thousand empty tables in an array, and the count they add; the count
-- itself depends on what the host made before the script
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local t = {} for i = 1, 1000 do t[i] = {} end
print(count() - base)
t = nil
print(count() - base)
