-- package.path and package.cpath given other values: the first full
-- collection after that frees the default string each held
collectgarbage("stop")
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
collectgarbage()
collectgarbage()
local base = count()
package.path = "x"
collectgarbage()
print("path", count() - base)
package.cpath = "y"
collectgarbage()
print("cpath", count() - base)
