-- a deep recursion grows the stack and the call records; a collection
-- after it shrinks them back as PUC does: the count, read after full
-- collections, has what is left
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local function deep(n) if n == 0 then return count() end return (deep(n - 1)) end
local at_depth = deep(300)
local after = count()
print(at_depth - base, after - base)
local function deeper(n, acc) if n == 0 then return count() end return (deeper(n - 1, acc .. "")) end
print(deeper(2000, "") - base, count() - base)
