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
-- a dead temporary above the register each op writes
local function newtable()
  do local a, b, c = mk(), mk(), mk() end
  phase = "nt"
  local t = {}
  phase = "after"
  return t
end
newtable()
show("newtable")
local function concat(s)
  do local a, b = mk(), mk() end
  phase = "cc"
  local r = s .. "x" .. s
  phase = "after"
  return r
end
concat("y")
show("concat")
local function closure()
  do local a, b = mk(), mk() end
  phase = "cl"
  local f = function() return phase end
  phase = "after"
  return f
end
closure()
show("closure")
phase = "end"
