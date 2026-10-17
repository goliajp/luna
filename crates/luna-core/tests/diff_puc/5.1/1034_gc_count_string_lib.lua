-- the string library's results, across the sizes where PUC's buffer
-- changes how it holds them
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
for _, n in ipairs({1, 10, 100, 1000, 1023, 1024, 1025, 5000, 8191, 8192, 8193, 20000}) do
  keep[#keep + 1] = string.rep("ab", math.floor(n / 2) + 1)
  keep[#keep + 1] = string.format("%s-%d-%5.2f-%q", string.rep("x", n), n, n / 7, "q\n")
  keep[#keep + 1] = (string.rep("abc", math.floor(n / 3) + 1):gsub("b", "BB"))
  keep[#keep + 1] = string.upper(string.rep("z", n))
end
print(#keep, count() - base)
keep = nil
print(count() - base)
