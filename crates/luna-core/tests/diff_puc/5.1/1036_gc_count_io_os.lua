-- io.read of long lines and whole files, and os.date
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local name = os.tmpname()
local f = io.open(name, "w")
for _, n in ipairs({0, 10, 1023, 1024, 1025, 8191, 8192, 8193, 30000}) do f:write(string.rep("L", n), "\n") end
f:close()
local base = count()
local keep = {}
f = io.open(name)
for line in f:lines() do keep[#keep + 1] = line end
f:close()
f = io.open(name)
keep[#keep + 1] = f:read("*a")
f:close()
f = io.open(name)
keep[#keep + 1] = f:read(5000)
f:close()
keep[#keep + 1] = os.date("!%Y-%m-%d %H:%M:%S", 0)
keep[#keep + 1] = os.date("!*t", 86400)
print(#keep, count() - base)
os.remove(name)
