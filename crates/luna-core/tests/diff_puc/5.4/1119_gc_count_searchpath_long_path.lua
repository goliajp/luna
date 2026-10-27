-- package.searchpath and require's Lua searcher over a path longer than
-- LUAL_BUFFERSIZE: the file names go into the buffer's box, a
-- to-be-closed slot that the C function's return closes once its results
-- are pushed; the box is garbage from then on, and a full collection
-- frees it
collectgarbage("stop")
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
local file = os.tmpname()
local h = assert(io.open(file, "w"))
h:write("return 1")
h:close()
local path = string.rep("/nonexistent-dir/x;", 70) .. file
collectgarbage()
local base = count()
local found = package.searchpath("m", path)
local made = count() - base
collectgarbage()
print("searchpath", found == file, made, count() - base)
package.path = path
collectgarbage()
base = count()
local r = require("m")
made = count() - base
collectgarbage()
print("require", r, made, count() - base)
os.remove(file)
