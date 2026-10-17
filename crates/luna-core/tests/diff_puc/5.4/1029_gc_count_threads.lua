-- many threads at different depths: their states, stacks and call
-- records, before and after half of them go
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local ts = {}
for i = 1, 100 do
  local co = coroutine.create(function(n)
    local function rec(k) if k == 0 then coroutine.yield() else rec(k - 1) end end
    rec(n)
  end)
  coroutine.resume(co, i % 17)
  ts[i] = co
end
print(count() - base)
for i = 1, 100, 2 do ts[i] = nil end
print(count() - base)
