-- rule: lvm.luaV_execute.1 lgc.luaC_condGC.1
-- what dies at each point the collector may run, with a full collection at
-- every such point: each object logs, as it is finalized, the phase then
local log, phase = {}, "start"
local function mk()
  local f = function() log[#log + 1] = phase end
  if newproxy then
    local o = newproxy(true)
    getmetatable(o).__gc = f
    return o
  end
  return setmetatable({}, {__gc = f})
end
local function show(name)
  print(name, table.concat(log, " "))
  log = {}
end
-- inside library functions: a __tostring that allocates, string.format
-- calling it, table.concat and gsub with a function
local obj = setmetatable({}, {__tostring = function()
  do local a = mk() end
  phase = "tostring"
  return "obj" .. tostring(#log)
end})
local function fmt()
  phase = "fmt"
  -- 5.1's %s takes only strings and numbers
  local o = _VERSION == "Lua 5.1" and tostring(obj) or obj
  local s = string.format("%s|%s", o, "x")
  phase = "after"
  return s
end
fmt()
show("format")
local function gs()
  do local a = mk() end
  phase = "gsub"
  local s = ("abc"):gsub("%w", function(c) return c .. c end)
  phase = "concat"
  local t = table.concat({"a", "b", "c"}, ",")
  phase = "after"
  return s, t
end
gs()
show("gsub")
phase = "end"
