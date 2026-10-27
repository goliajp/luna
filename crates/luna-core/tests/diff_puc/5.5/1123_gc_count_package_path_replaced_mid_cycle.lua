-- package.path given another value halfway through a collection cycle,
-- after the cycle traversed the package table: the default string was
-- marked already and lives until the next cycle
local v = _VERSION
if v == "Lua 5.4" then
  collectgarbage("incremental", 1000, 4, 1)
elseif v == "Lua 5.5" then
  collectgarbage("incremental")
  collectgarbage("param", "pause", 1000)
  collectgarbage("param", "stepsize", 256)
else
  collectgarbage("stop")
end
local function count() return collectgarbage("count") * 1024 end
local keep = {}
for i = 1, 300 do keep[i] = {} end
collectgarbage()
collectgarbage()
local n = 0
repeat n = n + 1 until collectgarbage("step", 0)
for _ = 1, math.floor(n / 2) do collectgarbage("step", 0) end
local base = count()
package.path = "x"
repeat until collectgarbage("step", 0)
print("cycle", string.format("%.0f", count() - base))
collectgarbage()
print("next", string.format("%.0f", count() - base))
