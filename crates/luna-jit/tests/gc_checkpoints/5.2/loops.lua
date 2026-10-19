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
-- allocations in loops a JIT compiles, and a function compiled whole
-- that makes a table on each level of a self-recursion
local function loop()
  local keep = {}
  for i = 1, 300 do
    local t = {i}
    if i % 100 == 0 then keep[#keep + 1] = mk(); keep[#keep] = nil end
    phase = "loop" .. math.floor(i / 100)
  end
  phase = "after"
  return keep
end
for _ = 1, 3 do loop() end
show("loop")
local function rec(n)
  local t = {n}
  if n > 0 then return rec(n - 1) end
  return t
end
local function recur()
  for i = 1, 200 do
    if i % 50 == 0 then do local a = mk() end; phase = "rec" .. math.floor(i / 50) end
    rec(2)
  end
  phase = "after"
end
recur()
show("recur")
phase = "end"
