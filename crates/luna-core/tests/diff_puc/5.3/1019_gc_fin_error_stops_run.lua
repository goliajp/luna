-- rule: lgc.GCTM.10
local ran = {}
local function mk(name, fail)
  local o
  if newproxy then o = newproxy(true); getmetatable(o).__gc = function() ran[#ran+1] = name; if fail then error(name) end end
  else o = setmetatable({}, {__gc = function() ran[#ran+1] = name; if fail then error(name) end end}) end
  return o
end
do local a, b, c = mk("a", true), mk("b", true), mk("c", false) end
print(pcall(collectgarbage))
print(table.concat(ran, ","))
print(pcall(collectgarbage))
print(table.concat(ran, ","))
print(pcall(collectgarbage))
print(table.concat(ran, ","))
