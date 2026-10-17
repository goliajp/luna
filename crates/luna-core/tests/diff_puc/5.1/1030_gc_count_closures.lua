-- closures made in loops, some sharing an upvalue, some the same each
-- time: the count after full collections
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local keep = {}
for i = 1, 50 do
  local shared = i
  keep[#keep + 1] = function() return shared end
  keep[#keep + 1] = function() return shared + 1 end
  keep[#keep + 1] = function() return 1 end
end
print(count() - base)
keep = nil
print(count() - base)
