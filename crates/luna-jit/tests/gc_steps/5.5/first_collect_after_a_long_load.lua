-- the first full collection after a load long enough that PUC, in the
-- generational mode lua.c sets, makes a minor collection while it parses
collectgarbage("stop")
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
local file = os.tmpname()
local mode = "c"
if mode ~= "c" then
  local h = assert(io.open(file, "w"))
  h:write("return 1")
  h:close()
end
local base = count()
if mode == "a" then package.searchpath("m", file) end
collectgarbage()
print(mode, count() - base)
os.remove(file)
