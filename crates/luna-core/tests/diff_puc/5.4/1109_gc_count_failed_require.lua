-- a module that fails to compile: the Lua searcher loads it with
-- luaL_loadfilex, a C function with no call level of its own, inside the
-- searcher's frame, whose stack top is what the shrink of the load's
-- protected call sees
collectgarbage("stop")
local function deep(n) if n > 0 then return deep(n - 1) + 1 end return 0 end
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
local file = os.tmpname()
local h = assert(io.open(file, "w"))
h:write("x x")
h:close()
package.path = file
deep(300)
local base = count()
local ok = pcall(require, "m9")
local c = count() - base
os.remove(file)
print("require", ok, c)
