-- rule: lgc.GCTM.14
-- only the explicit collections below run, whatever the automatic pacing
collectgarbage("stop")
local function mk(f)
  if newproxy then local o = newproxy(true); getmetatable(o).__gc = f; return o end
  return setmetatable({}, {__gc = f})
end
mk(function() error("boom") end)
-- the chunk name and line differ with how the file is run
local ok, e = xpcall(collectgarbage, function(m) return "handled: " .. tostring(m) end)
print(ok, (tostring(e):gsub("[^%s(]+:%d+: ", "")))
