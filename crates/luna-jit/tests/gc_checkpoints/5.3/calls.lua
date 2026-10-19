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
-- calls: Lua to Lua, Lua to C, through metamethods and pcall, and one
-- that grows the stack
local function leaf(x) return x end
local function calls()
  do local a, b = mk(), mk() end
  phase = "lua"
  leaf(1)
  phase = "c"
  rawequal(1, 2)
  phase = "after"
end
calls()
show("calls")
local function meta()
  do local a = mk() end
  phase = "index"
  local v = setmetatable({}, {__index = function(t, k) return k end}).z
  phase = "pcall"
  do local b = mk() end
  pcall(leaf, 2)
  phase = "after"
  return v
end
meta()
show("meta")
local function big(n)
  local a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13, a14, a15, a16
  local b1, b2, b3, b4, b5, b6, b7, b8, b9, b10, b11, b12, b13, b14, b15, b16
  if n > 0 then return big(n - 1) end
  return n
end
local function grow()
  do local a = mk() end
  phase = "grow"
  big(40)
  phase = "after"
end
grow()
show("grow")
phase = "end"
