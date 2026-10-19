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
-- a runtime error and a new thread
local function err()
  do local a = mk() end
  phase = "err"
  local ok = pcall(function() local x = nil; return x + 1 end)
  phase = "after"
  return ok
end
err()
show("error")
local function thread()
  do local a = mk() end
  phase = "co"
  local co = coroutine.create(function() return 1 end)
  phase = "after"
  return co
end
thread()
show("thread")
phase = "end"
