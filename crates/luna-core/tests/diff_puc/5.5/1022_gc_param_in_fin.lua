-- rule: lbaselib.luaB_collectgarbage.8
-- only the explicit collections below run, whatever the automatic pacing
collectgarbage("stop")
local r = {}
setmetatable({}, {__gc = function()
  r[#r+1] = tostring(collectgarbage("param", "pause"))
  r[#r+1] = tostring(collectgarbage("param", "pause", 100))
  r[#r+1] = tostring(collectgarbage("count"))
  r[#r+1] = tostring(collectgarbage("step"))
  r[#r+1] = tostring(collectgarbage("isrunning"))
  r[#r+1] = tostring(collectgarbage("incremental"))
  r[#r+1] = tostring(collectgarbage())
end})
collectgarbage()
print(table.concat(r, " "), collectgarbage("param", "pause"))
