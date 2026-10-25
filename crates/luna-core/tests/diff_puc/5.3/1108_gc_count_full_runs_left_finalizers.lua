-- a finalizer's error ends a full collection with another finalizer still
-- due; the next full collection runs that one first (luaC_fullgc runs the
-- cycle left in GCScallfin to its pause), then its own cycle, which frees
-- the object finalized
collectgarbage("stop")
local log = {}
local function mk(name, fail, big)
  return setmetatable({big and string.rep(name, 4000)}, {__gc = function()
    log[#log + 1] = name
    if fail then error(name) end
  end})
end
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
do local a, b = mk("a", false, true), mk("b", true) end
local base = count()
local ok, e = pcall(collectgarbage)
print(ok, (string.gsub(e, "%(.-:%d+: ", "(")))
print(table.concat(log, ","), count() - base)
print(pcall(collectgarbage))
print(table.concat(log, ","), count() - base)
print(pcall(collectgarbage))
print(table.concat(log, ","), count() - base)
