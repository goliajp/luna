-- concatenations of many pieces, of numbers, short and long results
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
for i = 1, 30 do
  keep[i] = "a" .. i .. "b" .. (i / 3) .. string.rep("x", i * 3) .. "end"
end
print(count() - base)
local s = ""
for i = 1, 200 do s = s .. i end
print(#s, count() - base)
keep, s = nil, nil
print(count() - base)
