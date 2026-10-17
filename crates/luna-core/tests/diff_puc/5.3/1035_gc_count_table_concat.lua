-- table.concat over pieces of many sizes and counts
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
for _, n in ipairs({0, 1, 5, 50, 500, 3000}) do
  local t = {}
  for i = 1, n do t[i] = i % 3 == 0 and i or string.rep("p", i % 40) end
  keep[#keep + 1] = table.concat(t, ", ")
end
print(#keep, count() - base)
