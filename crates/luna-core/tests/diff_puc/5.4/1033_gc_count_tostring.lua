-- tostring of every kind of value, many times
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local vals = {1, 2.5, -0.0, 1e300, "s", true, nil, false}
local acc = 0
for i = 1, 200 do
  for j = 1, 8 do acc = acc + #tostring(vals[j]) end
  acc = acc + #tostring(i) + #tostring(i + 0.5)
end
print(acc, count() - base)
